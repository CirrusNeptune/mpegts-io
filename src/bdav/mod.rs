use super::{read_bitfield, AppDetails, MpegTsParser, Packet, PesUnitObject, Result, SliceReader};
use modular_bitfield_msb::prelude::*;

mod mobj;
use mobj::MObjCmd;

mod pg;
use pg::PgSegmentData;

#[bitfield]
#[derive(Debug)]
pub struct BdavPacketHeader {
    pub cpi: B2,
    pub timestamp: B30,
}

#[derive(Debug)]
pub struct BdavPacket<'a, D> {
    pub header: BdavPacketHeader,
    pub packet: Packet<'a, D>,
}

#[derive(Debug)]
pub enum BdavErrorDetails {
    UnknownIgSegmentType(u8),
}

pub trait BdavAppDetails: AppDetails<AppErrorDetails = BdavErrorDetails> {}

#[derive(Default, Debug)]
pub struct DefaultBdavAppDetails;

impl AppDetails for DefaultBdavAppDetails {
    type AppErrorDetails = BdavErrorDetails;

    fn new_pes_unit_data(pid: u16, unit_length: usize) -> Option<Box<dyn PesUnitObject<Self>>> {
        match pid {
            0x1200..=0x121f | 0x1400..=0x141f | 0x1800 => {
                Some(Box::new(PgSegmentData::new(unit_length)))
            }
            _ => None,
        }
    }
}

impl BdavAppDetails for DefaultBdavAppDetails {}

#[derive(Default)]
pub struct BdavParser<D: BdavAppDetails = DefaultBdavAppDetails>(MpegTsParser<D>);

impl<D: BdavAppDetails> BdavParser<D> {
    pub fn parse<'a>(&mut self, packet: &'a [u8; 192]) -> Result<BdavPacket<'a, D>, D> {
        let mut reader = SliceReader::new(packet);
        let header = read_bitfield!(reader, BdavPacketHeader);
        Ok(BdavPacket {
            header,
            packet: self.0.parse_internal(reader)?,
        })
    }
}
