use super::{MpegTsParser, Packet, Result, SliceReader};
use modular_bitfield_msb::prelude::*;

#[bitfield]
#[derive(Debug)]
pub struct BDAVPacketHeader {
    pub cpi: B2,
    pub timestamp: B30,
}

#[derive(Debug)]
pub struct BDAVPacket<'a> {
    pub header: BDAVPacketHeader,
    pub packet: Packet<'a>,
}

#[derive(Default)]
pub struct BDAVParser(MpegTsParser);

impl BDAVParser {
    pub fn parse<'a>(&mut self, packet: &'a [u8; 192]) -> Result<BDAVPacket<'a>> {
        let mut reader = SliceReader::new(packet);
        let header = BDAVPacketHeader::from_bytes(*reader.read_array_ref::<4>()?);
        Ok(BDAVPacket {
            header,
            packet: self.0.parse_internal(reader)?,
        })
    }
}
