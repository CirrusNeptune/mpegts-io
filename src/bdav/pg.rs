use super::{
    mobj::MObjCmd, read_bitfield, AppDetails, BdavAppDetails, BdavErrorDetails, MpegTsParser,
    PesUnitObject, SliceReader,
};
use crate::{ErrorDetails, Result};
use log::warn;
use modular_bitfield_msb::prelude::*;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;

#[derive(Debug, Default, Copy, Clone)]
pub(crate) struct PgsPaletteEntry {
    pub y: u8,
    pub cr: u8,
    pub cb: u8,
    pub t: u8,
}

#[derive(Debug)]
pub(crate) struct PgsPalette {
    pub id: u8,
    pub version: u8,
    pub entries: Box<[PgsPaletteEntry; 256]>,
}

impl PgsPalette {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
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

#[derive(Debug)]
pub(crate) struct PgsObject {}

impl PgsObject {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        Ok(Self {})
    }
}

#[derive(Debug)]
pub(crate) struct PgsPgComposition {}

impl PgsPgComposition {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        Ok(Self {})
    }
}

#[derive(Debug)]
pub(crate) struct PgsWindow {}

impl PgsWindow {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        Ok(Self {})
    }
}

#[derive(Debug, Copy, Clone, PartialEq, FromPrimitive)]
enum FrameRate {
    Invalid,
    Drop24,
    NonDrop24,
    NonDrop25,
    Drop30,
    NonDrop50,
    Drop60,
}

impl Default for FrameRate {
    fn default() -> Self {
        FrameRate::Invalid
    }
}

#[derive(Debug)]
pub(crate) struct PgVideoDescriptor {
    video_width: u16,
    video_height: u16,
    frame_rate: FrameRate,
}

impl PgVideoDescriptor {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let video_width = reader.read_be_u16()?;
        let video_height = reader.read_be_u16()?;
        let frame_rate = FromPrimitive::from_u8(reader.read_u8()? >> 4).unwrap_or_default();
        Ok(Self {
            video_width,
            video_height,
            frame_rate,
        })
    }
}

#[derive(Debug)]
pub(crate) struct PgCompositionDescriptor {
    number: u16,
    state: u8,
}

impl PgCompositionDescriptor {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let number = reader.read_be_u16()?;
        let state = reader.read_u8()?;
        Ok(Self { number, state })
    }
}

#[derive(Debug)]
pub(crate) struct PgSequenceDescriptor {
    pub first_in_seq: bool,
    pub last_in_seq: bool,
}

impl PgSequenceDescriptor {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let bits = reader.read_u8()?;
        Ok(Self {
            first_in_seq: bits & 0x80 != 0,
            last_in_seq: bits & 0x40 != 0,
        })
    }
}

#[bitfield]
#[derive(Debug)]
pub(crate) struct UoMask {
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

#[derive(Debug)]
pub(crate) struct IgWindow {
    pub id: u8,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl IgWindow {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
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

#[derive(Debug)]
pub(crate) struct PgCrop {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl PgCrop {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let x = reader.read_be_u16()?;
        let y = reader.read_be_u16()?;
        let w = reader.read_be_u16()?;
        let h = reader.read_be_u16()?;
        Ok(Self { x, y, w, h })
    }
}

#[derive(Debug)]
pub(crate) struct PgCompositionObject {
    pub object_id_ref: u16,
    pub window_id_ref: u8,
    pub forced_on_flag: bool,
    pub x: u16,
    pub y: u16,
    pub crop: Option<PgCrop>,
}

impl PgCompositionObject {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
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

#[derive(Debug)]
pub(crate) struct IgEffect {
    pub duration: u32,
    pub palette_id_ref: u8,
    pub composition_objects: Vec<PgCompositionObject>,
}

impl IgEffect {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
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

#[derive(Debug)]
pub(crate) struct IgEffectSequence {
    pub windows: Vec<IgWindow>,
    pub effects: Vec<IgEffect>,
}

impl IgEffectSequence {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let num_windows = reader.read_u8()?;
        let mut windows = Vec::with_capacity(num_windows as usize);
        for _ in 0..num_windows {
            windows.push(IgWindow::parse(reader)?);
        }
        let num_effects = reader.read_u8()?;
        let mut effects = Vec::with_capacity(num_effects as usize);
        for _ in 0..num_effects {
            effects.push(IgEffect::parse(reader)?);
        }
        Ok(Self { windows, effects })
    }
}

#[derive(Debug)]
pub(crate) struct IgButton {
    pub id: u16,
    pub numeric_select_value: u16,
    pub auto_action_flag: bool,
    pub x_pos: u16,
    pub y_pos: u16,
    pub upper_button_id_ref: u16,
    pub lower_button_id_ref: u16,
    pub left_button_id_ref: u16,
    pub right_button_id_ref: u16,
    pub normal_start_object_id_ref: u16,
    pub normal_end_object_id_ref: u16,
    pub normal_repeat_flag: bool,
    pub selected_sound_id_ref: u8,
    pub selected_start_object_id_ref: u16,
    pub selected_end_object_id_ref: u16,
    pub selected_repeat_flag: bool,
    pub activated_sound_id_ref: u8,
    pub activated_start_object_id_ref: u16,
    pub activated_end_object_id_ref: u16,
    pub nav_cmds: Vec<MObjCmd>,
}

impl IgButton {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
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

#[derive(Debug)]
pub(crate) struct IgBog {
    pub default_valid_button_id_ref: u16,
    pub buttons: Vec<IgButton>,
}

impl IgBog {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
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

#[derive(Debug)]
pub(crate) struct IgPage {
    pub id: u8,
    pub version: u8,
    pub uo_mask: UoMask,
    pub in_effects: IgEffectSequence,
    pub out_effects: IgEffectSequence,
    pub animation_frame_rate_code: u8,
    pub default_selected_button_id_ref: u16,
    pub default_activated_button_id_ref: u16,
    pub palette_id_ref: u8,
    pub bogs: Vec<IgBog>,
}

impl IgPage {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
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

#[derive(Debug)]
pub(crate) struct IgInteractiveComposition {
    pub stream_model: bool,
    pub ui_model: bool,
    pub composition_timeout_pts: Option<u64>,
    pub selection_timeout_pts: Option<u64>,
    pub user_timeout_duration: u32,
    pub pages: Vec<IgPage>,
}

impl IgInteractiveComposition {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let data_len = reader.read_be_u24()?;
        let mut sub_reader = reader.new_sub_reader(data_len as usize)?;
        let model_bits = sub_reader.read_u8()?;
        let stream_model = model_bits & 0x80 != 0;
        let (composition_timeout_pts, selection_timeout_pts) = if !stream_model {
            let composition_timeout_pts = sub_reader.read_be_u33()?;
            let selection_timeout_pts = sub_reader.read_be_u33()?;
            (Some(composition_timeout_pts), Some(selection_timeout_pts))
        } else {
            (None, None)
        };
        let user_timeout_duration = sub_reader.read_be_u24()?;
        let num_pages = sub_reader.read_u8()?;
        let mut pages = Vec::with_capacity(num_pages as usize);
        for _ in 0..num_pages {
            pages.push(IgPage::parse(&mut sub_reader)?);
        }
        if sub_reader.remaining_len() != 0 {
            warn!("entire ig interactive composition not read");
        }
        Ok(Self {
            stream_model,
            ui_model: model_bits & 0x40 != 0,
            composition_timeout_pts,
            selection_timeout_pts,
            user_timeout_duration,
            pages,
        })
    }
}

#[derive(Debug)]
pub(crate) struct PgsIgComposition {
    pub video_descriptor: PgVideoDescriptor,
    pub composition_descriptor: PgCompositionDescriptor,
    pub sequence_descriptor: PgSequenceDescriptor,
    pub interactive_composition: IgInteractiveComposition,
}

impl PgsIgComposition {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let video_descriptor = PgVideoDescriptor::parse(reader)?;
        let composition_descriptor = PgCompositionDescriptor::parse(reader)?;
        let sequence_descriptor = PgSequenceDescriptor::parse(reader)?;
        let interactive_composition = IgInteractiveComposition::parse(reader)?;
        Ok(Self {
            video_descriptor,
            composition_descriptor,
            sequence_descriptor,
            interactive_composition,
        })
    }
}

#[derive(Debug)]
pub(crate) struct PgsEndOfDisplay {}

impl PgsEndOfDisplay {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        Ok(Self {})
    }
}

#[derive(Debug)]
pub(crate) struct TgsDialogStyle {}

impl TgsDialogStyle {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        Ok(Self {})
    }
}

#[derive(Debug)]
pub(crate) struct TgsDialogPresentation {}

impl TgsDialogPresentation {
    fn parse<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        Ok(Self {})
    }
}

macro_rules! pg_segment_data {
    // Exit rule.
    (
        @collect_unitary_variants
        ($(,)*) -> ($($var:ident = $val:expr,)*)
    ) => {
        #[derive(Debug)]
        pub(crate) enum PgSegmentData {
            Raw(Vec<u8>),
            $($var($var),)*
        }

        fn parse_pg_segment_data<D: BdavAppDetails>(reader: &mut SliceReader<D>) -> Result<PgSegmentData, D> {
            let seg_type = reader.read_u8()?;
            let seg_length = reader.read_be_u16()?;
            let mut seg_reader = reader.new_sub_reader(seg_length as usize)?;

            let ret = match seg_type {
                $($val => Ok(PgSegmentData::$var($var::parse(&mut seg_reader)?)),)*
                _ => Err(seg_reader.make_error(ErrorDetails::<D>::AppError(BdavErrorDetails::UnknownIgSegmentType(seg_type))))
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
        ($var:ident = $val:expr, $($tail:tt)*) -> ($($var_names:tt)*)
    ) => {
        pg_segment_data! {
            @collect_unitary_variants
            ($($tail)*) -> ($($var_names)* $var = $val,)
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
    PgsPalette = 0x14,
    PgsObject = 0x15,
    PgsPgComposition = 0x16,
    PgsWindow = 0x17,
    PgsIgComposition = 0x18,
    PgsEndOfDisplay = 0x80,
    TgsDialogStyle = 0x81,
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
            *self = parse_pg_segment_data(&mut SliceReader::new(data.as_slice()))?;
            Ok(())
        } else {
            panic!("PgSegmentData must be raw before finishing")
        }
    }
}
