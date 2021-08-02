use super::{
    read_bitfield, ErrorDetails, MpegTsParser, Packet, PesUnitObject, PesUnitObjectFactory, Result,
    SliceReader,
};
use log::warn;
use modular_bitfield_msb::prelude::*;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::ops::Range;
use std::rc::Rc;

#[derive(Debug, Default, Copy, Clone)]
struct PgsPaletteEntry {
    pub y: u8,
    pub cr: u8,
    pub cb: u8,
    pub t: u8,
}

#[derive(Debug)]
struct PgsPalette {
    pub id: u8,
    pub version: u8,
    pub entries: Box<[PgsPaletteEntry; 256]>,
}

impl PgsPalette {
    fn parse(reader: &mut SliceReader) -> Result<Self> {
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
struct PgsObject {}

impl PgsObject {
    fn parse(reader: &mut SliceReader) -> Result<Self> {
        Ok(Self {})
    }
}

#[derive(Debug)]
struct PgsPgComposition {}

impl PgsPgComposition {
    fn parse(reader: &mut SliceReader) -> Result<Self> {
        Ok(Self {})
    }
}

#[derive(Debug)]
struct PgsWindow {}

impl PgsWindow {
    fn parse(reader: &mut SliceReader) -> Result<Self> {
        Ok(Self {})
    }
}

#[derive(Debug)]
struct PgVideoDescriptor {
    video_width: u16,
    video_height: u16,
    frame_rate: u8,
}

impl PgVideoDescriptor {
    fn parse(reader: &mut SliceReader) -> Result<Self> {
        let video_width = reader.read_be_u16()?;
        let video_height = reader.read_be_u16()?;
        let frame_rate = reader.read_u8()?;
        Ok(Self {
            video_width,
            video_height,
            frame_rate,
        })
    }
}

#[derive(Debug)]
struct PgCompositionDescriptor {
    number: u16,
    state: u8,
}

impl PgCompositionDescriptor {
    fn parse(reader: &mut SliceReader) -> Result<Self> {
        let number = reader.read_be_u16()?;
        let state = reader.read_u8()?;
        Ok(Self { number, state })
    }
}

#[derive(Debug)]
struct IgInteractiveComposition {}

impl IgInteractiveComposition {
    fn parse(reader: &mut SliceReader) -> Result<Self> {
        Ok(Self {})
    }
}

#[derive(Debug)]
struct PgsIgComposition {
    pub video_descriptor: PgVideoDescriptor,
    pub composition_descriptor: PgCompositionDescriptor,
    pub interactive_composition: IgInteractiveComposition,
}

impl PgsIgComposition {
    fn parse(reader: &mut SliceReader) -> Result<Self> {
        let video_descriptor = PgVideoDescriptor::parse(reader)?;
        let composition_descriptor = PgCompositionDescriptor::parse(reader)?;
        let interactive_composition = IgInteractiveComposition::parse(reader)?;
        Ok(Self {
            video_descriptor,
            composition_descriptor,
            interactive_composition,
        })
    }
}

#[derive(Debug)]
struct PgsEndOfDisplay {}

impl PgsEndOfDisplay {
    fn parse(reader: &mut SliceReader) -> Result<Self> {
        Ok(Self {})
    }
}

#[derive(Debug)]
struct TgsDialogStyle {}

impl TgsDialogStyle {
    fn parse(reader: &mut SliceReader) -> Result<Self> {
        Ok(Self {})
    }
}

#[derive(Debug)]
struct TgsDialogPresentation {}

impl TgsDialogPresentation {
    fn parse(reader: &mut SliceReader) -> Result<Self> {
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
        enum PgSegmentData {
            Raw(Vec<u8>),
            $($var($var),)*
        }

        fn parse_pg_segment_data(reader: &mut SliceReader) -> Result<PgSegmentData> {
            let seg_type = reader.read_u8()?;
            let seg_length = reader.read_be_u16()?;
            let mut seg_reader = reader.new_sub_reader(seg_length as usize)?;

            let ret = match seg_type {
                $($val => Ok(PgSegmentData::$var($var::parse(&mut seg_reader)?)),)*
                _ => Err(seg_reader.make_error(ErrorDetails::PesError(String::from(
                    "Unknown ig segment type",
                ))))
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

impl PesUnitObject for PgSegmentData {
    fn extend_from_slice(&mut self, slice: &[u8]) {
        if let PgSegmentData::Raw(data) = self {
            data.extend_from_slice(slice);
        } else {
            panic!("IgSegmentData must be raw before finishing")
        }
    }

    fn finish(&mut self, pid: u16, parser: &mut MpegTsParser) -> Result<()> {
        if let PgSegmentData::Raw(data) = self {
            *self = parse_pg_segment_data(&mut SliceReader::new(data.as_slice()))?;
            Ok(())
        } else {
            panic!("IgSegmentData must be raw before finishing")
        }
    }
}

#[derive(Default)]
struct PgDataFactory;
impl PesUnitObjectFactory for PgDataFactory {
    fn construct(&self, pid: u16, capacity: usize) -> Box<dyn PesUnitObject> {
        /* IgSegmentData always starts raw and is transformed to parsed form at end */
        Box::new(PgSegmentData::Raw(Vec::with_capacity(capacity)))
    }
}

#[bitfield]
#[derive(Debug)]
pub struct BdavPacketHeader {
    pub cpi: B2,
    pub timestamp: B30,
}

#[derive(Debug)]
pub struct BdavPacket<'a> {
    pub header: BdavPacketHeader,
    pub packet: Packet<'a>,
}

pub struct BdavParser(MpegTsParser);

impl Default for BdavParser {
    fn default() -> Self {
        let mut new_self = Self(MpegTsParser::default());

        let pg_factory = Rc::new(PgDataFactory::default());
        /* Program Graphics */
        new_self.register_pes_unit_factory_iter(0x1200..=0x121f, pg_factory.clone());
        /* Interactive Graphics */
        new_self.register_pes_unit_factory_iter(0x1400..=0x141f, pg_factory.clone());
        /* Text Subtitles */
        new_self.register_pes_unit_factory(0x1800, pg_factory);

        new_self
    }
}

impl BdavParser {
    pub fn register_pes_unit_factory(&mut self, pid: u16, factory: Rc<dyn PesUnitObjectFactory>) {
        self.0.register_pes_unit_factory(pid, factory);
    }

    pub fn register_pes_unit_factory_iter<I: Iterator<Item = u16>>(
        &mut self,
        pids: I,
        factory: Rc<dyn PesUnitObjectFactory>,
    ) {
        self.0.register_pes_unit_factory_iter(pids, factory);
    }

    pub fn parse<'a>(&mut self, packet: &'a [u8; 192]) -> Result<BdavPacket<'a>> {
        let mut reader = SliceReader::new(packet);
        let header = read_bitfield!(reader, BdavPacketHeader);
        Ok(BdavPacket {
            header,
            packet: self.0.parse_internal(reader)?,
        })
    }
}
