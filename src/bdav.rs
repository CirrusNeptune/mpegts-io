use super::{MpegTsParser, Packet, Result, SliceReader};
use modular_bitfield_msb::prelude::*;

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

#[derive(Default)]
pub struct BdavParser(MpegTsParser);

impl BdavParser {
    pub fn parse<'a>(&mut self, packet: &'a [u8; 192]) -> Result<BdavPacket<'a>> {
        let mut reader = SliceReader::new(packet);
        let header = BdavPacketHeader::from_bytes(*reader.read_array_ref::<4>()?);
        Ok(BdavPacket {
            header,
            packet: self.0.parse_internal(reader)?,
        })
    }
}
