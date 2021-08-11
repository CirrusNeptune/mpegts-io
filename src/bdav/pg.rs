//! Module for working with declarative program (PG) and interactive (IG) graphics streams used
//! in Blu-Ray subtitles and menus.

use super::{
    from_primitive_map_err, mobj::MObjCmd, read_bitfield, BdavAppDetails, BdavErrorDetails,
    BdavParserStorage, MpegTsParser, PesUnitObject, SliceReader,
};
use crate::{ErrorDetails, Result};
use log::warn;
use modular_bitfield_msb::prelude::*;
use num_derive::FromPrimitive;
use smallvec::SmallVec;
use std::cmp::min;
use std::fmt::{Debug, Formatter};

/// A YCbCrA palette entry.
#[derive(Debug, Default, Copy, Clone)]
pub struct PgsPaletteEntry {
    /// Luminance
    pub y: u8,
    /// Red Chrominance
    pub cr: u8,
    /// Blue Chrominance
    pub cb: u8,
    /// Alpha
    pub t: u8,
}

/// A palette object that defines colors for [`PgsObject`] objects.
#[derive(Debug)]
pub struct PgsPalette {
    /// Palette ID
    pub id: u8,
    /// Format version
    pub version: u8,
    /// 256 palette entries
    pub entries: Box<[PgsPaletteEntry; 256]>,
}

impl PgsPalette {
    fn parse<D: BdavAppDetails>(
        reader: &mut SliceReader<D>,
        storage: &mut BdavParserStorage,
    ) -> Result<Self, D> {
        let id = reader.read_u8()?;
        let version = reader.read_u8()?;
        let mut out = PgsPalette {
            id,
            version,
            entries: Box::new([PgsPaletteEntry::default(); 256]),
        };

        while reader.remaining_len() > 0 {
            let entry = &mut out.entries[reader.read_u8()? as usize];
            entry.y = reader.read_u8()?;
            entry.cr = reader.read_u8()?;
            entry.cb = reader.read_u8()?;
            entry.t = reader.read_u8()?;
        }

        Ok(out)
    }
}

/// Final parsed data of [`PgsObject`].
pub struct PgsObjectData {
    /// Object width.
    pub width: u16,
    /// Object height.
    pub height: u16,
    /// Object RLE data.
    pub data: Vec<u8>,
}

impl Debug for PgsObjectData {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgsObjectData")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("data.len()", &self.data.len())
            .finish()
    }
}

impl PgsObjectData {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let width = reader.read_be_u16()?;
        let height = reader.read_be_u16()?;

        let mut data = Vec::new();
        data.extend_from_slice(reader.read_to_end()?);

        Ok(Self {
            width,
            height,
            data,
        })
    }
}

/// An indexed-color image used within a graphics composition.
#[derive(Debug)]
pub struct PgsObject {
    /// Object ID
    pub id: u16,
    /// Format version
    pub version: u8,
    /// Flags that indicate the position of a segment split across multiple units.
    pub sequence_descriptor: PgSequenceDescriptor,
    /// Parsed data after segment fragments are reassembled.
    pub data: Option<PgsObjectData>,
}

impl PgsObject {
    fn parse<D: BdavAppDetails>(
        reader: &mut SliceReader<D>,
        storage: &mut BdavParserStorage,
    ) -> Result<Self, D> {
        let id = reader.read_be_u16()?;
        let version = reader.read_u8()?;
        let sequence_descriptor = PgSequenceDescriptor::parse(reader)?;
        let key = (id, version);

        if sequence_descriptor.first_in_seq && sequence_descriptor.last_in_seq {
            // Single-fragment case; immediately parse data.
            let length = reader.read_be_u24()? as usize;
            if reader.remaining_len() > length {
                warn!("Unexpectedly long PgsObject data; truncating");
            }
            Ok(Self {
                id,
                version,
                sequence_descriptor,
                data: Some(PgsObjectData::parse(reader)?),
            })
        } else if sequence_descriptor.first_in_seq {
            // First fragment of many.
            if storage.pending_obj_segments.contains_key(&key) {
                warn!("Discarding pending PgsObject({}, {})", id, version);
            }
            let length = reader.read_be_u24()?;
            let mut data = Vec::with_capacity(length as usize);
            if reader.remaining_len() > data.capacity() {
                warn!("Unexpectedly long PgsObject data; truncating");
            }
            data.extend_from_slice(reader.read(min(reader.remaining_len(), data.capacity()))?);
            storage.pending_obj_segments.insert(key, data);
            Ok(Self {
                id,
                version,
                sequence_descriptor,
                data: None,
            })
        } else if !sequence_descriptor.first_in_seq && !sequence_descriptor.last_in_seq {
            // Intermediate fragment of many.
            match storage.pending_obj_segments.get_mut(&key) {
                Some(mut data) => {
                    if data.len() + reader.remaining_len() > data.capacity() {
                        warn!("Unexpectedly long PgsObject data; truncating");
                    }
                    data.extend_from_slice(
                        reader.read(min(reader.remaining_len(), data.capacity() - data.len()))?,
                    );
                    Ok(Self {
                        id,
                        version,
                        sequence_descriptor,
                        data: None,
                    })
                }
                None => Err(reader.make_error(ErrorDetails::AppError(
                    BdavErrorDetails::NonStartedPgsObject,
                ))),
            }
        } else {
            // Final fragment of many.
            match storage.pending_obj_segments.remove(&key) {
                Some(mut data) => {
                    if data.len() + reader.remaining_len() > data.capacity() {
                        warn!("Unexpectedly long PgsObject data; truncating");
                    }
                    data.extend_from_slice(
                        reader.read(min(reader.remaining_len(), data.capacity() - data.len()))?,
                    );
                    Ok(Self {
                        id,
                        version,
                        sequence_descriptor,
                        data: Some(PgsObjectData::parse(&mut SliceReader::new(&data))?),
                    })
                }
                None => Err(reader.make_error(ErrorDetails::AppError(
                    BdavErrorDetails::NonStartedPgsObject,
                ))),
            }
        }
    }
}

/// A program graphics composition.
#[derive(Debug)]
pub struct PgsPgComposition {
    /// Viewport and frame rate information.
    pub video_descriptor: PgVideoDescriptor,
    /// Information about the sequence of PES units that make up the composition.
    pub composition_descriptor: PgCompositionDescriptor,
    /// Indicates that only the palette has changed.
    pub palette_update_flag: bool,
    /// [`PgsPalette`] object ID.
    pub palette_id_ref: u8,
    /// Positioned [`PgsObject`] elements of the composition.
    pub composition_objects: Vec<PgCompositionObject>,
}

impl PgsPgComposition {
    fn parse<D: BdavAppDetails>(
        reader: &mut SliceReader<D>,
        storage: &mut BdavParserStorage,
    ) -> Result<Self, D> {
        let video_descriptor = PgVideoDescriptor::parse(reader)?;
        let composition_descriptor = PgCompositionDescriptor::parse(reader)?;
        let palette_update_flag = reader.read_u8()? & 0x80 != 0;
        let palette_id_ref = reader.read_u8()?;

        let num_composition_objects = reader.read_u8()?;
        let mut composition_objects = Vec::with_capacity(num_composition_objects as usize);
        for _ in 0..num_composition_objects {
            composition_objects.push(PgCompositionObject::parse(reader)?);
        }

        Ok(Self {
            video_descriptor,
            composition_descriptor,
            palette_update_flag,
            palette_id_ref,
            composition_objects,
        })
    }
}

/// A collection of windows for referencing by [`PgCompositionObject`] objects.
#[derive(Debug)]
pub struct PgsWindow {
    /// Windows in the collection.
    pub windows: Vec<PgWindow>,
}

impl PgsWindow {
    fn parse<D: BdavAppDetails>(
        reader: &mut SliceReader<D>,
        storage: &mut BdavParserStorage,
    ) -> Result<Self, D> {
        let num_windows = reader.read_u8()?;
        let mut windows = Vec::with_capacity(num_windows as usize);
        for _ in 0..num_windows {
            windows.push(PgWindow::parse(reader)?);
        }
        Ok(Self { windows })
    }
}

/// Frame rate used for timing in an [`PgsIgComposition`].
#[derive(Debug, Copy, Clone, PartialEq, FromPrimitive)]
pub enum FrameRate {
    /// Unspecified frame rate; animated effects not possible.
    Invalid,
    /// 24000/1001 Hz
    Drop24,
    /// 24 Hz
    NonDrop24,
    /// 25 Hz
    NonDrop25,
    /// 30000/1001 Hz
    Drop30,
    /// 50 Hz
    NonDrop50,
    /// 60000/1001 Hz
    Drop60,
}

/// Video viewport information for the graphics composition.
#[derive(Debug)]
pub struct PgVideoDescriptor {
    /// Width in pixels.
    video_width: u16,
    /// Height in pixels.
    video_height: u16,
    /// Frame rate.
    frame_rate: FrameRate,
}

impl PgVideoDescriptor {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let video_width = reader.read_be_u16()?;
        let video_height = reader.read_be_u16()?;
        let frame_rate = from_primitive_map_err(reader.read_u8()? >> 4, |v| {
            reader.make_error(ErrorDetails::AppError(BdavErrorDetails::UnknownFrameRate(
                v,
            )))
        })?;
        Ok(Self {
            video_width,
            video_height,
            frame_rate,
        })
    }
}

/// Streaming information about a PG PES unit.
#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Hash, Clone, FromPrimitive)]
pub enum PgCompositionUnitState {
    /// An object that adds to the composition being streamed.
    Incremental,
    /// First palette of a new set (clearing out the old one).
    NewPalette,
    /// Entirely new composition that clears all loaded composition objects.
    EpochStart,
}

/// Information about the sequence of PES units that make up a composition.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct PgCompositionDescriptor {
    /// Unique identifier of composition for assembling unit fragments.
    pub number: u16,
    /// Streaming information about a PG PES unit.
    pub state: PgCompositionUnitState,
}

impl PgCompositionDescriptor {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let number = reader.read_be_u16()?;
        let state = from_primitive_map_err(reader.read_u8()? >> 6, |v| {
            reader.make_error(ErrorDetails::AppError(
                BdavErrorDetails::UnknownPgCompositionUnitState(v),
            ))
        })?;
        Ok(Self { number, state })
    }
}

/// Flags that indicate the position of a segment split across multiple units.
#[derive(Debug)]
pub struct PgSequenceDescriptor {
    /// Is first in sequence.
    pub first_in_seq: bool,
    /// Is last in sequence.
    pub last_in_seq: bool,
}

impl PgSequenceDescriptor {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let bits = reader.read_u8()?;
        Ok(Self {
            first_in_seq: bits & 0x80 != 0,
            last_in_seq: bits & 0x40 != 0,
        })
    }
}

/// User operations mask.
#[bitfield]
#[derive(Debug)]
pub struct UoMask {
    pub menu_call: bool,
    pub title_search: bool,
    pub chapter_search: bool,
    pub time_search: bool,
    pub skip_to_next_point: bool,
    pub skip_to_prev_point: bool,
    pub play_firstplay: bool,
    pub stop: bool,
    pub pause_on: bool,
    pub pause_off: bool,
    pub still_off: bool,
    pub forward: bool,
    pub backward: bool,
    pub resume: bool,
    pub move_up: bool,
    pub move_down: bool,
    pub move_left: bool,
    pub move_right: bool,
    pub select: bool,
    pub activate: bool,
    pub select_and_activate: bool,
    pub primary_audio_change: bool,
    #[skip]
    pub unused: bool,
    pub angle_change: bool,
    pub popup_on: bool,
    pub popup_off: bool,
    pub pg_enable_disable: bool,
    pub pg_change: bool,
    pub secondary_video_enable_disable: bool,
    pub secondary_video_change: bool,
    pub secondary_audio_enable_disable: bool,
    pub secondary_audio_change: bool,
    #[skip]
    pub unused2: bool,
    pub pip_pg_change: bool,
    #[skip]
    pub unused3: B30,
}

/// Sub-rectangle in a composition for positioning [`PgCompositionObject`] objects in an
/// [`IgEffectSequence`] or for [`PgsWindow`] objects within a [`PgsPgComposition`].
#[derive(Debug)]
pub struct PgWindow {
    /// Window ID.
    pub id: u8,
    /// X pos.
    pub x: u16,
    /// Y pos.
    pub y: u16,
    /// Width.
    pub width: u16,
    /// Height.
    pub height: u16,
}

impl PgWindow {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let id = reader.read_u8()?;
        let x = reader.read_be_u16()?;
        let y = reader.read_be_u16()?;
        let width = reader.read_be_u16()?;
        let height = reader.read_be_u16()?;
        Ok(Self {
            id,
            x,
            y,
            width,
            height,
        })
    }
}

/// Clipping dimensions for a [`PgCompositionObject`]
#[derive(Debug)]
pub struct PgCrop {
    /// X Pos.
    pub x: u16,
    /// Y Pos.
    pub y: u16,
    /// Width.
    pub w: u16,
    /// Height.
    pub h: u16,
}

impl PgCrop {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let x = reader.read_be_u16()?;
        let y = reader.read_be_u16()?;
        let w = reader.read_be_u16()?;
        let h = reader.read_be_u16()?;
        Ok(Self { x, y, w, h })
    }
}

/// A positioned graphical element of a composition.
#[derive(Debug)]
pub struct PgCompositionObject {
    /// Object ID.
    pub object_id_ref: u16,
    /// Window ID.
    pub window_id_ref: u8,
    /// Forced display.
    pub forced_on_flag: bool,
    /// X Pos.
    pub x: u16,
    /// Y Pos.
    pub y: u16,
    /// Optional clipping dimensions.
    pub crop: Option<PgCrop>,
}

impl PgCompositionObject {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let object_id_ref = reader.read_be_u16()?;
        let window_id_ref = reader.read_u8()?;
        let bits = reader.read_u8()?;
        let x = reader.read_be_u16()?;
        let y = reader.read_be_u16()?;
        let crop = if bits & 0x80 != 0 {
            Some(PgCrop::parse(reader)?)
        } else {
            None
        };
        Ok(Self {
            object_id_ref,
            window_id_ref,
            forced_on_flag: bits & 0x40 != 0,
            x,
            y,
            crop,
        })
    }
}

/// A set of [`PgCompositionObject`] objects that are displayed for a fixed duration.
#[derive(Debug)]
pub struct IgEffect {
    /// Display duration in 90kHz ticks.
    pub duration: u32,
    /// Palette ID.
    pub palette_id_ref: u8,
    /// Contained composition objects.
    pub composition_objects: Vec<PgCompositionObject>,
}

impl IgEffect {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let duration = reader.read_be_u24()?;
        let palette_id_ref = reader.read_u8()?;
        let num_composition_objects = reader.read_u8()?;
        let mut composition_objects = Vec::with_capacity(num_composition_objects as usize);
        for _ in 0..num_composition_objects {
            composition_objects.push(PgCompositionObject::parse(reader)?);
        }
        Ok(Self {
            duration,
            palette_id_ref,
            composition_objects,
        })
    }
}

/// Collects windows and effects to animate hide/show transitions of a composition.
#[derive(Debug)]
pub struct IgEffectSequence {
    /// Windows for composition objects contained in effects.
    pub windows: Vec<PgWindow>,
    /// Timed composition objects for the effect sequence.
    pub effects: Vec<IgEffect>,
}

impl IgEffectSequence {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let num_windows = reader.read_u8()?;
        let mut windows = Vec::with_capacity(num_windows as usize);
        for _ in 0..num_windows {
            windows.push(PgWindow::parse(reader)?);
        }
        let num_effects = reader.read_u8()?;
        let mut effects = Vec::with_capacity(num_effects as usize);
        for _ in 0..num_effects {
            effects.push(IgEffect::parse(reader)?);
        }
        Ok(Self { windows, effects })
    }
}

/// Complete definition of an interactive button.
#[derive(Debug)]
pub struct IgButton {
    /// Button ID.
    pub id: u16,
    /// Remote control number pad equivalent.
    pub numeric_select_value: u16,
    /// Auto activate when selected.
    pub auto_action_flag: bool,
    /// X Pos.
    pub x_pos: u16,
    /// Y Pos.
    pub y_pos: u16,
    /// Button ID to navigate up.
    pub upper_button_id_ref: u16,
    /// Button ID to navigate down.
    pub lower_button_id_ref: u16,
    /// Button ID to navigate left.
    pub left_button_id_ref: u16,
    /// Button ID to navigate right.
    pub right_button_id_ref: u16,
    /// Ranged start of animated button frame object IDs (normal state).
    pub normal_start_object_id_ref: u16,
    /// Ranged end of animated button frame object IDs (normal state).
    pub normal_end_object_id_ref: u16,
    /// Loop animation (normal state).
    pub normal_repeat_flag: bool,
    /// Sound ID when selected.
    pub selected_sound_id_ref: u8,
    /// Ranged start of animated button frame object IDs (selected state).
    pub selected_start_object_id_ref: u16,
    /// Ranged end of animated button frame object IDs (selected state).
    pub selected_end_object_id_ref: u16,
    /// Loop animation (selected state).
    pub selected_repeat_flag: bool,
    /// Sound ID when activated.
    pub activated_sound_id_ref: u8,
    /// Ranged start of animated button frame object IDs (activated state).
    pub activated_start_object_id_ref: u16,
    /// Ranged end of animated button frame object IDs (activated state).
    pub activated_end_object_id_ref: u16,
    /// MObj commands executed when button is activated.
    pub nav_cmds: Vec<MObjCmd>,
}

impl IgButton {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let id = reader.read_be_u16()?;
        let numeric_select_value = reader.read_be_u16()?;
        let auto_action_flag = reader.read_u8()? & 0x80 != 0;
        let x_pos = reader.read_be_u16()?;
        let y_pos = reader.read_be_u16()?;
        let upper_button_id_ref = reader.read_be_u16()?;
        let lower_button_id_ref = reader.read_be_u16()?;
        let left_button_id_ref = reader.read_be_u16()?;
        let right_button_id_ref = reader.read_be_u16()?;
        let normal_start_object_id_ref = reader.read_be_u16()?;
        let normal_end_object_id_ref = reader.read_be_u16()?;
        let normal_repeat_flag = reader.read_u8()? & 0x80 != 0;
        let selected_sound_id_ref = reader.read_u8()?;
        let selected_start_object_id_ref = reader.read_be_u16()?;
        let selected_end_object_id_ref = reader.read_be_u16()?;
        let selected_repeat_flag = reader.read_u8()? & 0x80 != 0;
        let activated_sound_id_ref = reader.read_u8()?;
        let activated_start_object_id_ref = reader.read_be_u16()?;
        let activated_end_object_id_ref = reader.read_be_u16()?;
        let num_nav_cmds = reader.read_be_u16()?;
        let mut nav_cmds = Vec::with_capacity(num_nav_cmds as usize);
        for _ in 0..num_nav_cmds {
            nav_cmds.push(MObjCmd::parse(reader)?);
        }
        Ok(Self {
            id,
            numeric_select_value,
            auto_action_flag,
            x_pos,
            y_pos,
            upper_button_id_ref,
            lower_button_id_ref,
            left_button_id_ref,
            right_button_id_ref,
            normal_start_object_id_ref,
            normal_end_object_id_ref,
            normal_repeat_flag,
            selected_sound_id_ref,
            selected_start_object_id_ref,
            selected_end_object_id_ref,
            selected_repeat_flag,
            activated_sound_id_ref,
            activated_start_object_id_ref,
            activated_end_object_id_ref,
            nav_cmds,
        })
    }
}

/// Logical grouping of buttons used to implement selection hierarchies.
#[derive(Debug)]
pub struct IgBog {
    /// Default button ID within group.
    pub default_valid_button_id_ref: u16,
    /// Buttons in group.
    pub buttons: Vec<IgButton>,
}

impl IgBog {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let default_valid_button_id_ref = reader.read_be_u16()?;
        let num_buttons = reader.read_u8()?;
        let mut buttons = Vec::with_capacity(num_buttons as usize);
        for _ in 0..num_buttons {
            buttons.push(IgButton::parse(reader)?);
        }
        Ok(Self {
            default_valid_button_id_ref,
            buttons,
        })
    }
}

/// Collection of buttons such that only one is visible at a time.
#[derive(Debug)]
pub struct IgPage {
    /// Page ID.
    pub id: u8,
    /// Format version.
    pub version: u8,
    /// User operation mask.
    pub uo_mask: UoMask,
    /// Animated show effects.
    pub in_effects: IgEffectSequence,
    /// Animated hide effects.
    pub out_effects: IgEffectSequence,
    /// Additional frames to delay next frame of animated buttons.
    pub animation_frame_rate_code: u8,
    /// Default selected button ID.
    pub default_selected_button_id_ref: u16,
    /// Default activated button ID.
    pub default_activated_button_id_ref: u16,
    /// Palette ID.
    pub palette_id_ref: u8,
    /// Button groups.
    pub bogs: Vec<IgBog>,
}

impl IgPage {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let id = reader.read_u8()?;
        let version = reader.read_u8()?;
        let uo_mask = read_bitfield!(reader, UoMask);
        let in_effects = IgEffectSequence::parse(reader)?;
        let out_effects = IgEffectSequence::parse(reader)?;
        let animation_frame_rate_code = reader.read_u8()?;
        let default_selected_button_id_ref = reader.read_be_u16()?;
        let default_activated_button_id_ref = reader.read_be_u16()?;
        let palette_id_ref = reader.read_u8()?;
        let num_bogs = reader.read_u8()?;
        let mut bogs = Vec::with_capacity(num_bogs as usize);
        for _ in 0..num_bogs {
            bogs.push(IgBog::parse(reader)?);
        }
        Ok(Self {
            id,
            version,
            uo_mask,
            in_effects,
            out_effects,
            animation_frame_rate_code,
            default_selected_button_id_ref,
            default_activated_button_id_ref,
            palette_id_ref,
            bogs,
        })
    }
}

/// UI Model used in an [`IgInteractiveComposition`].
#[repr(u8)]
#[derive(Debug)]
pub enum IgUiModel {
    /// Always on menu.
    AlwaysOn,
    /// Popup menu.
    Popup,
}

/// Interactive UI composition containing pages of buttons.
#[derive(Debug)]
pub struct IgInteractiveComposition {
    /// TODO: Figure this out
    pub stream_model: bool,
    /// Type of menu UI.
    pub ui_model: IgUiModel,
    /// TODO: Figure this out
    pub composition_timeout_pts: Option<u64>,
    /// TODO: Figure this out
    pub selection_timeout_pts: Option<u64>,
    /// Inactivity time to wait before hiding popup or returning to page 0 in 90kHz ticks.
    pub user_timeout_duration: u32,
    /// Pages of composition
    pub pages: Vec<IgPage>,
}

impl IgInteractiveComposition {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let model_bits = reader.read_u8()?;
        let stream_model = model_bits & 0x80 != 0;
        let (composition_timeout_pts, selection_timeout_pts) = if !stream_model {
            let composition_timeout_pts = reader.read_be_u33()?;
            let selection_timeout_pts = reader.read_be_u33()?;
            (Some(composition_timeout_pts), Some(selection_timeout_pts))
        } else {
            (None, None)
        };
        let user_timeout_duration = reader.read_be_u24()?;
        let num_pages = reader.read_u8()?;
        let mut pages = Vec::with_capacity(num_pages as usize);
        for _ in 0..num_pages {
            pages.push(IgPage::parse(reader)?);
        }
        if reader.remaining_len() != 0 {
            warn!("entire ig interactive composition not read");
        }
        Ok(Self {
            stream_model,
            ui_model: if model_bits & 0x40 != 0 {
                IgUiModel::Popup
            } else {
                IgUiModel::AlwaysOn
            },
            composition_timeout_pts,
            selection_timeout_pts,
            user_timeout_duration,
            pages,
        })
    }
}

/// Interactive composition unit containing top-level metadata.
#[derive(Debug)]
pub struct PgsIgComposition {
    /// Viewport and frame rate information.
    pub video_descriptor: PgVideoDescriptor,
    /// Information about the sequence of PES units that make up the composition.
    pub composition_descriptor: PgCompositionDescriptor,
    /// Flags that indicate the position of a segment split across multiple units.
    pub sequence_descriptor: PgSequenceDescriptor,
    /// Parsed data after segment fragments are reassembled.
    pub interactive_composition: Option<IgInteractiveComposition>,
}

impl PgsIgComposition {
    fn parse<D: BdavAppDetails>(
        reader: &mut SliceReader<D>,
        storage: &mut BdavParserStorage,
    ) -> Result<Self, D> {
        let video_descriptor = PgVideoDescriptor::parse(reader)?;
        let composition_descriptor = PgCompositionDescriptor::parse(reader)?;
        let sequence_descriptor = PgSequenceDescriptor::parse(reader)?;

        if sequence_descriptor.first_in_seq && sequence_descriptor.last_in_seq {
            // Single-fragment case; immediately parse data.
            let length = reader.read_be_u24()? as usize;
            if reader.remaining_len() > length {
                warn!("Unexpectedly long PgsIgComposition data; truncating");
            }
            Ok(Self {
                video_descriptor,
                composition_descriptor,
                sequence_descriptor,
                interactive_composition: Some(IgInteractiveComposition::parse(reader)?),
            })
        } else if sequence_descriptor.first_in_seq {
            // First fragment of many.
            if storage
                .pending_ig_segments
                .contains_key(&composition_descriptor)
            {
                warn!(
                    "Discarding pending PgsIgComposition({:?})",
                    composition_descriptor
                );
            }
            let length = reader.read_be_u24()?;
            let mut data = Vec::with_capacity(length as usize);
            if reader.remaining_len() > data.capacity() {
                warn!("Unexpectedly long PgsIgComposition data; truncating");
            }
            data.extend_from_slice(reader.read(min(reader.remaining_len(), data.capacity()))?);
            storage
                .pending_ig_segments
                .insert(composition_descriptor.clone(), data);
            Ok(Self {
                video_descriptor,
                composition_descriptor,
                sequence_descriptor,
                interactive_composition: None,
            })
        } else if !sequence_descriptor.first_in_seq && !sequence_descriptor.last_in_seq {
            // Intermediate fragment of many.
            match storage.pending_ig_segments.get_mut(&composition_descriptor) {
                Some(mut data) => {
                    if data.len() + reader.remaining_len() > data.capacity() {
                        warn!("Unexpectedly long PgsIgComposition data; truncating");
                    }
                    data.extend_from_slice(
                        reader.read(min(reader.remaining_len(), data.capacity() - data.len()))?,
                    );
                    Ok(Self {
                        video_descriptor,
                        composition_descriptor,
                        sequence_descriptor,
                        interactive_composition: None,
                    })
                }
                None => Err(reader.make_error(ErrorDetails::AppError(
                    BdavErrorDetails::NonStartedPgsIgComposition,
                ))),
            }
        } else {
            // Final fragment of many.
            match storage.pending_ig_segments.remove(&composition_descriptor) {
                Some(mut data) => {
                    if data.len() + reader.remaining_len() > data.capacity() {
                        warn!("Unexpectedly long PgsIgComposition data; truncating");
                    }
                    data.extend_from_slice(
                        reader.read(min(reader.remaining_len(), data.capacity() - data.len()))?,
                    );
                    Ok(Self {
                        video_descriptor,
                        composition_descriptor,
                        sequence_descriptor,
                        interactive_composition: Some(IgInteractiveComposition::parse(
                            &mut SliceReader::new(&data),
                        )?),
                    })
                }
                None => Err(reader.make_error(ErrorDetails::AppError(
                    BdavErrorDetails::NonStartedPgsIgComposition,
                ))),
            }
        }
    }
}

/// Marks final PES unit and player is now be ready to display composition.
#[derive(Debug)]
pub struct PgsEndOfDisplay {}

impl PgsEndOfDisplay {
    fn parse<D: BdavAppDetails>(
        reader: &mut SliceReader<D>,
        storage: &mut BdavParserStorage,
    ) -> Result<Self, D> {
        Ok(Self {})
    }
}

/// Filled background rectangle for presenting text.
#[derive(Debug)]
pub struct TgRegionInfo {
    /// Rectangle region.
    pub region: TgRect,
    /// Background palette index.
    pub background_color: u8,
}

impl TgRegionInfo {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let region = TgRect::parse(reader)?;
        let background_color = reader.read_u8()?;
        reader.skip(1)?;
        Ok(Self {
            region,
            background_color,
        })
    }
}

/// Rectangle dimensions.
#[derive(Debug)]
pub struct TgRect {
    /// X Pos.
    pub xpos: u16,
    /// Y Pos.
    pub ypos: u16,
    /// Width.
    pub width: u16,
    /// Height.
    pub height: u16,
}

impl TgRect {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let xpos = reader.read_be_u16()?;
        let ypos = reader.read_be_u16()?;
        let width = reader.read_be_u16()?;
        let height = reader.read_be_u16()?;
        Ok(Self {
            xpos,
            ypos,
            width,
            height,
        })
    }
}

/// Text flow.
#[repr(u8)]
#[derive(Debug, FromPrimitive)]
pub enum TgTextFlow {
    /// Left-to-right, top-to-bottom.
    LeftRight = 1,
    /// Right-to-left, top-to-bottom.
    RightLeft = 2,
    /// Top-to-bottom, right-to-left.
    TopBottom = 3,
}

/// Text horizontal alignment.
#[repr(u8)]
#[derive(Debug, FromPrimitive)]
pub enum TgHAlign {
    /// Left alignment.
    Left = 1,
    /// Center alignment.
    Center = 2,
    /// Right alignment.
    Right = 3,
}

/// Text vertical alignment.
#[repr(u8)]
#[derive(Debug, FromPrimitive)]
pub enum TgVAlign {
    /// Top alignment.
    Top = 1,
    /// Middle alignment.
    Middle = 2,
    /// Bottom alignment.
    Bottom = 3,
}

/// Text font style bits.
#[bitfield]
#[derive(Debug)]
pub struct TgFontStyle {
    #[skip]
    pub padding: B5,
    pub bold: bool,
    pub italic: bool,
    pub outline_border: bool,
}

/// Text outline thickness.
#[repr(u8)]
#[derive(Debug, FromPrimitive)]
pub enum TgOutlineThickness {
    /// Thin.
    Thin = 1,
    /// Medium.
    Medium = 2,
    /// Thick.
    Thick = 3,
}

/// Style parameters for a text region.
#[derive(Debug)]
pub struct TgRegionStyle {
    /// Region style ID.
    pub region_style_id: u8,
    /// Background rectangle info,
    pub region_info: TgRegionInfo,
    /// Text box rectangle.
    pub text_box: TgRect,
    /// Text flow.
    pub text_flow: TgTextFlow,
    /// Text horizontal alignment.
    pub text_halign: TgHAlign,
    /// Text vertical alignment.
    pub text_valign: TgVAlign,
    /// Line spacing.
    pub line_space: u8,
    /// Font ID.
    pub font_id_ref: u8,
    /// Font style bits.
    pub font_style: TgFontStyle,
    /// Font size.
    pub font_size: u8,
    /// Font color palette index.
    pub font_color: u8,
    /// Outline color palette index.
    pub outline_color: u8,
    /// Outline thickness.
    pub outline_thickness: TgOutlineThickness,
}

impl TgRegionStyle {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let region_style_id = reader.read_u8()?;
        let region_info = TgRegionInfo::parse(reader)?;
        let text_box = TgRect::parse(reader)?;
        let text_flow = from_primitive_map_err(reader.read_u8()?, |v| {
            reader.make_error(ErrorDetails::AppError(BdavErrorDetails::UnknownTgTextFlow(
                v,
            )))
        })?;
        let text_halign = from_primitive_map_err(reader.read_u8()?, |v| {
            reader.make_error(ErrorDetails::AppError(BdavErrorDetails::UnknownTgHAlign(v)))
        })?;
        let text_valign = from_primitive_map_err(reader.read_u8()?, |v| {
            reader.make_error(ErrorDetails::AppError(BdavErrorDetails::UnknownTgVAlign(v)))
        })?;
        let line_space = reader.read_u8()?;
        let font_id_ref = reader.read_u8()?;
        let font_style = read_bitfield!(reader, TgFontStyle);
        let font_size = reader.read_u8()?;
        let font_color = reader.read_u8()?;
        let outline_color = reader.read_u8()?;
        let outline_thickness = from_primitive_map_err(reader.read_u8()?, |v| {
            reader.make_error(ErrorDetails::AppError(
                BdavErrorDetails::UnknownTgOutlineThickness(v),
            ))
        })?;
        Ok(Self {
            region_style_id,
            region_info,
            text_box,
            text_flow,
            text_halign,
            text_valign,
            line_space,
            font_id_ref,
            font_style,
            font_size,
            font_color,
            outline_color,
            outline_thickness,
        })
    }
}

/// TODO: Document me.
#[allow(missing_docs)]
#[derive(Debug)]
pub struct TgUserStyle {
    pub user_style_id: u8,
    pub region_hpos_delta: i16,
    pub region_vpos_delta: i16,
    pub text_box_hpos_delta: i16,
    pub text_box_vpos_delta: i16,
    pub text_box_width_delta: i16,
    pub text_box_height_delta: i16,
    pub font_size_delta: i8,
    pub line_space_delta: i8,
}

impl TgUserStyle {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let user_style_id = reader.read_u8()?;
        let region_hpos_delta = reader.read_be_sm16()?;
        let region_vpos_delta = reader.read_be_sm16()?;
        let text_box_hpos_delta = reader.read_be_sm16()?;
        let text_box_vpos_delta = reader.read_be_sm16()?;
        let text_box_width_delta = reader.read_be_sm16()?;
        let text_box_height_delta = reader.read_be_sm16()?;
        let font_size_delta = reader.read_sm8()?;
        let line_space_delta = reader.read_sm8()?;
        Ok(Self {
            user_style_id,
            region_hpos_delta,
            region_vpos_delta,
            text_box_hpos_delta,
            text_box_vpos_delta,
            text_box_width_delta,
            text_box_height_delta,
            font_size_delta,
            line_space_delta,
        })
    }
}

fn read_palette_entries<D: BdavAppDetails>(
    reader: &mut SliceReader<D>,
) -> Result<Box<[PgsPaletteEntry; 256]>, D> {
    let mut palette_entries = Box::new([PgsPaletteEntry::default(); 256]);
    let num_palette_entries = reader.read_be_u16()? / 5;
    for _ in 0..num_palette_entries {
        let entry = &mut palette_entries[reader.read_u8()? as usize];
        entry.y = reader.read_u8()?;
        entry.cr = reader.read_u8()?;
        entry.cb = reader.read_u8()?;
        entry.t = reader.read_u8()?;
    }
    Ok(palette_entries)
}

/// Container of text styles.
#[derive(Debug)]
pub struct TgDialogStyle {
    /// Unknown
    pub player_style_flag: bool,
    /// Text region styles.
    pub region_styles: Vec<TgRegionStyle>,
    /// Text user styles.
    pub user_styles: Vec<TgUserStyle>,
    /// 256 palette entries
    pub palette_entries: Box<[PgsPaletteEntry; 256]>,
}

impl TgDialogStyle {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let player_style_flag = reader.read_be_u16()? & 0x8000 != 0;

        let num_region_styles = reader.read_u8()?;
        let mut region_styles = Vec::with_capacity(num_region_styles as usize);
        for _ in 0..num_region_styles {
            region_styles.push(TgRegionStyle::parse(reader)?);
        }

        let num_user_styles = reader.read_u8()?;
        let mut user_styles = Vec::with_capacity(num_user_styles as usize);
        for _ in 0..num_user_styles {
            user_styles.push(TgUserStyle::parse(reader)?);
        }

        let palette_entries = read_palette_entries(reader)?;

        Ok(Self {
            player_style_flag,
            region_styles,
            user_styles,
            palette_entries,
        })
    }
}

/// Set of dialog styles.
#[derive(Debug)]
pub struct TgsDialogStyle {
    /// Styles of the dialogs.
    pub style: TgDialogStyle,
    /// Total count of dialogs in stream.
    pub num_dialogs: u16,
}

impl TgsDialogStyle {
    fn parse<D: BdavAppDetails>(
        reader: &mut SliceReader<D>,
        storage: &mut BdavParserStorage,
    ) -> Result<Self, D> {
        let style = TgDialogStyle::parse(reader)?;
        let num_dialogs = reader.read_be_u16()?;
        Ok(Self { style, num_dialogs })
    }
}

/// A presentation of one dialog region.
#[derive(Debug)]
pub struct TgDialogRegion {
    /// Unknown
    pub continuous_present_flag: bool,
    /// Unknown
    pub forced_on_flag: bool,
    /// Region style ID.
    pub region_style_id_ref: u8,
    /// Data of presentation (TODO parse formatting tags)
    pub data: Vec<u8>,
}

impl TgDialogRegion {
    fn parse<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let bits = reader.read_u8()?;
        let continuous_present_flag = bits & 0x80 != 0;
        let forced_on_flag = bits & 0x40 != 0;
        let region_style_id_ref = reader.read_u8()?;
        let data_length = reader.read_be_u16()? as usize;
        let mut data = Vec::with_capacity(data_length);
        data.extend_from_slice(reader.read(data_length)?);
        Ok(Self {
            continuous_present_flag,
            forced_on_flag,
            region_style_id_ref,
            data,
        })
    }
}

/// Presentable text instance.
#[derive(Debug)]
pub struct TgsDialogPresentation {
    /// Start timecode.
    pub start_pts: u64,
    /// End timecode.
    pub end_pts: u64,
    /// Optional palette update.
    pub palette_update: Option<Box<[PgsPaletteEntry; 256]>>,
    /// Up to 2 regions to present.
    pub regions: SmallVec<[TgDialogRegion; 2]>,
}

impl TgsDialogPresentation {
    fn parse<D: BdavAppDetails>(
        reader: &mut SliceReader<D>,
        storage: &mut BdavParserStorage,
    ) -> Result<Self, D> {
        let start_pts = reader.read_be_u33()?;
        let end_pts = reader.read_be_u33()?;
        let has_palette_update = reader.read_u8()? & 0x80 != 0;
        let palette_update = if has_palette_update {
            Some(read_palette_entries(reader)?)
        } else {
            None
        };
        let region_count = reader.read_u8()?;
        if region_count > 2 {
            warn!("too many TgDialogRegions");
        }
        let mut regions = SmallVec::new();
        for _ in 0..min(region_count, 2) {
            regions.push(TgDialogRegion::parse(reader)?);
        }
        Ok(Self {
            start_pts,
            end_pts,
            palette_update,
            regions,
        })
    }
}

macro_rules! pg_segment_data {
    // Exit rule.
    (
        @collect_unitary_variants
        ($(,)*) -> ($($(#[$vattr:meta])* $var:ident = $val:expr,)*)
    ) => {
        /// A PES unit that starts with raw data and is converted to parsed form at end.
        #[derive(Debug)]
        pub enum PgSegmentData {
            /// Unparsed PES payload data for accumulating packets.
            Raw(Vec<u8>),
            $($(#[$vattr])* $var($var),)*
        }

        fn parse_pg_segment_data<D: BdavAppDetails>(reader: &mut SliceReader<D>, storage: &mut BdavParserStorage) -> Result<PgSegmentData, D> {
            let seg_type = reader.read_u8()?;
            let seg_length = reader.read_be_u16()?;
            let mut seg_reader = reader.new_sub_reader(seg_length as usize)?;

            let ret = match seg_type {
                $($val => Ok(PgSegmentData::$var($var::parse(&mut seg_reader, storage)?)),)*
                _ => Err(seg_reader.make_error(ErrorDetails::<D>::AppError(BdavErrorDetails::UnknownPgSegmentType(seg_type))))
            };

            if seg_reader.remaining_len() > 0 {
                warn!("entire ig segment not read")
            }

            ret
        }
    };

    // Handle a variant.
    (
        @collect_unitary_variants
        ($(#[$vattr:meta])* $var:ident = $val:expr, $($tail:tt)*) -> ($($var_names:tt)*)
    ) => {
        pg_segment_data! {
            @collect_unitary_variants
            ($($tail)*) -> ($($var_names)* $(#[$vattr])* $var = $val,)
        }
    };

    // Entry rule.
    ($($body:tt)*) => {
        pg_segment_data! {
            @collect_unitary_variants
            ($($body)*,) -> ()
        }
    };
}

pg_segment_data! {
    /// Palette object.
    PgsPalette = 0x14,
    /// Graphical Object object.
    PgsObject = 0x15,
    /// Program Graphics Composition object.
    PgsPgComposition = 0x16,
    /// Program Graphics Window object.
    PgsWindow = 0x17,
    /// Interactive Graphics Composition object.
    PgsIgComposition = 0x18,
    /// End of display mark.
    PgsEndOfDisplay = 0x80,
    /// TODO: Document me.
    TgsDialogStyle = 0x81,
    /// TODO: Document me.
    TgsDialogPresentation = 0x82,
}

impl PgSegmentData {
    pub(crate) fn new(unit_length: usize) -> Self {
        PgSegmentData::Raw(Vec::with_capacity(unit_length))
    }
}

impl<D: BdavAppDetails> PesUnitObject<D> for PgSegmentData {
    fn extend_from_slice(&mut self, slice: &[u8]) {
        if let PgSegmentData::Raw(data) = self {
            data.extend_from_slice(slice);
        } else {
            panic!("PgSegmentData must be raw before finishing")
        }
    }

    fn finish(&mut self, pid: u16, parser: &mut MpegTsParser<D>) -> Result<(), D> {
        if let PgSegmentData::Raw(data) = self {
            *self = parse_pg_segment_data(
                &mut SliceReader::new(data.as_slice()),
                &mut parser.app_parser_storage,
            )?;
            Ok(())
        } else {
            panic!("PgSegmentData must be raw before finishing")
        }
    }
}
