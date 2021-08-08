use super::{
    parse_timestamp, pts_format_args, read_bitfield, AppDetails, ErrorDetails, MpegTsParser,
    Payload, PayloadUnitObject, Result, SliceReader,
};
use log::warn;
use modular_bitfield_msb::prelude::*;
use std::fmt::{Arguments, Debug, DebugStruct, Formatter};
use std::rc::Rc;

#[bitfield]
#[derive(Debug)]
pub struct PesHeader {
    pub start_code: B24,
    pub stream_id: B8,
    pub packet_length: B16,
}

#[bitfield]
#[derive(Debug)]
pub struct PesOptionalHeader {
    pub marker_bits: B2,
    pub scrambling_control: B2,
    pub priority: bool,
    pub data_alignment_indicator: bool,
    pub copyright: bool,
    pub original: bool,
    pub has_pts: bool,
    pub has_dts: bool,
    pub escr: bool,
    pub es_rate: bool,
    pub dsm_trick_mode: bool,
    pub has_additional_copy_info: bool,
    pub has_crc: bool,
    pub has_extension: bool,
    pub additional_header_length: B8,
}

pub trait PesUnitObject<D: AppDetails>: Debug {
    fn extend_from_slice(&mut self, slice: &[u8]);
    fn finish(&mut self, pid: u16, parser: &mut MpegTsParser<D>) -> Result<(), D>;
}

#[derive(Default)]
struct RawPesData(Vec<u8>);

impl RawPesData {
    pub fn new(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }
}

impl Debug for RawPesData {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawPesData")
            .field("len", &self.0.len())
            .finish()
    }
}

impl<D: AppDetails> PesUnitObject<D> for RawPesData {
    fn extend_from_slice(&mut self, slice: &[u8]) {
        self.0.extend_from_slice(slice);
    }

    fn finish(&mut self, pid: u16, parser: &mut MpegTsParser<D>) -> Result<(), D> {
        Ok(())
    }
}

pub struct Pes<D> {
    pub header: PesHeader,
    pub optional_header: Option<PesOptionalHeader>,
    pub pts: Option<u64>,
    pub dts: Option<u64>,
    pub data: Box<dyn PesUnitObject<D>>,
}

impl<D> Pes<D> {
    pub fn new(
        header: PesHeader,
        optional_header: Option<PesOptionalHeader>,
        pts: Option<u64>,
        dts: Option<u64>,
        data: Box<dyn PesUnitObject<D>>,
    ) -> Self {
        Self {
            header,
            optional_header,
            pts,
            dts,
            data,
        }
    }
}

impl<D: AppDetails> PayloadUnitObject<D> for Pes<D> {
    fn extend_from_slice(&mut self, slice: &[u8]) {
        self.data.extend_from_slice(slice);
    }

    fn finish<'a>(mut self, pid: u16, parser: &mut MpegTsParser<D>) -> Result<Payload<'a, D>, D> {
        self.data.finish(pid, parser)?;
        Ok(Payload::Pes(self))
    }

    fn pending<'a>(&self) -> Result<Payload<'a, D>, D> {
        Ok(Payload::PesPending)
    }
}

fn fmt_pts_field(s: &mut DebugStruct, name: &str, ts: &Option<u64>) {
    if let Some(ts) = ts {
        s.field(name, &Some(pts_format_args!(ts)));
    } else {
        s.field(name, &Option::<Arguments>::None);
    }
}

impl<D> Debug for Pes<D> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("Pes");
        s.field("header", &self.header);
        s.field("optional_header", &self.optional_header);
        fmt_pts_field(&mut s, "pts", &self.pts);
        fmt_pts_field(&mut s, "dts", &self.dts);
        s.field("data", &self.data);
        s.finish()
    }
}

impl<D: AppDetails> MpegTsParser<D> {
    pub(crate) fn start_pes<'a>(
        &mut self,
        pid: u16,
        reader: &mut SliceReader<'a, D>,
    ) -> Result<Payload<'a, D>, D> {
        let pes_header = read_bitfield!(reader, PesHeader);
        let pes_length = pes_header.packet_length() as usize;
        let mut optional_length = 0;
        let mut pts = None;
        let mut dts = None;
        let pes_optional = if pes_length >= 3 && pes_header.stream_id() != 0xBF {
            let pes_optional = read_bitfield!(reader, PesOptionalHeader);
            let additional_length = pes_optional.additional_header_length() as usize;
            optional_length = 3 + additional_length;
            let mut o_reader = reader.new_sub_reader(additional_length)?;

            if pes_optional.has_pts() {
                if o_reader.remaining_len() < 5 {
                    warn!("Short read of PTS");
                    return Err(o_reader.make_error(ErrorDetails::<D>::BadPesHeader));
                }
                pts = Some(parse_timestamp(o_reader.read_array_ref::<5>()?));
            }

            if pes_optional.has_dts() {
                if o_reader.remaining_len() < 5 {
                    warn!("Short read of DTS");
                    return Err(o_reader.make_error(ErrorDetails::<D>::BadPesHeader));
                }
                dts = Some(parse_timestamp(o_reader.read_array_ref::<5>()?));
            }

            // TODO: Other fields
            Some(pes_optional)
        } else {
            None
        };

        let unit_length = pes_length - optional_length;

        let unit_data = if let Some(unit_data) = D::new_pes_unit_data(pid, unit_length) {
            unit_data
        } else {
            Box::new(RawPesData::new(unit_length))
        };

        self.start_payload_unit(
            Pes::new(pes_header, pes_optional, pts, dts, unit_data),
            unit_length,
            pid,
            reader,
        )
    }
}
