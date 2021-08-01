use super::{
    parse_timestamp, pts_format_args, ErrorDetails, MPEGTSParser, Payload, Result, SliceReader,
    SpanObject,
};
use log::warn;
use modular_bitfield_msb::prelude::*;
use std::fmt::{Debug, Formatter};

#[bitfield]
#[derive(Debug)]
pub struct PESHeader {
    pub start_code: B24,
    pub stream_id: B8,
    pub packet_length: B16,
}

#[bitfield]
#[derive(Debug)]
pub struct PESOptionalHeader {
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

pub struct PES {
    pub header: PESHeader,
    pub optional_header: Option<PESOptionalHeader>,
    pub pts: u64,
    pub dts: u64,
    pub data: Vec<u8>,
}

impl PES {
    pub fn new(
        capacity: usize,
        header: PESHeader,
        optional_header: Option<PESOptionalHeader>,
        pts: u64,
        dts: u64,
    ) -> Self {
        Self {
            header,
            optional_header,
            pts,
            dts,
            data: Vec::with_capacity(capacity),
        }
    }
}

impl SpanObject for PES {
    fn extend_from_slice(&mut self, slice: &[u8]) {
        self.data.extend_from_slice(slice);
    }

    fn finish<'a>(self, pid: u16, parser: &mut MPEGTSParser) -> Result<Payload<'a>> {
        Ok(Payload::PES(self))
    }

    fn pending<'a>(&self) -> Result<Payload<'a>> {
        Ok(Payload::PESPending)
    }
}

impl Debug for PES {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PES")
            .field("header", &self.header)
            .field("optional_header", &self.optional_header)
            .field("pts", &pts_format_args!(self.pts))
            .field("dts", &pts_format_args!(self.dts))
            .field("data.len()", &self.data.len())
            .finish()
    }
}

impl MPEGTSParser {
    pub(crate) fn start_pes<'a>(
        &mut self,
        pid: u16,
        reader: &mut SliceReader<'a>,
    ) -> Result<Payload<'a>> {
        let pes_header = PESHeader::from_bytes(*reader.read_array_ref::<6>()?);
        let pes_length = pes_header.packet_length() as usize;
        let mut optional_length = 0;
        let mut pts = 0;
        let mut dts = 0;
        let pes_optional = if pes_length >= 3 && pes_header.stream_id() != 0xBF {
            let pes_optional = PESOptionalHeader::from_bytes(*reader.read_array_ref::<3>()?);
            let additional_length = pes_optional.additional_header_length() as usize;
            optional_length = 3 + additional_length;
            let mut o_reader = reader.new_sub_reader(additional_length)?;

            if pes_optional.has_pts() {
                if o_reader.remaining_len() < 5 {
                    warn!("Short read of PTS");
                    return Err(o_reader.make_error(ErrorDetails::BadPESHeader));
                }
                pts = parse_timestamp(o_reader.read_array_ref::<5>()?);
            }

            if pes_optional.has_dts() {
                if o_reader.remaining_len() < 5 {
                    warn!("Short read of DTS");
                    return Err(o_reader.make_error(ErrorDetails::BadPESHeader));
                }
                dts = parse_timestamp(o_reader.read_array_ref::<5>()?);
            }

            // TODO: Other fields
            Some(pes_optional)
        } else {
            None
        };

        let span_length = pes_length - optional_length;
        self.start_span(
            PES::new(span_length, pes_header, pes_optional, pts, dts),
            span_length,
            pid,
            reader,
        )
    }
}
