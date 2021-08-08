//! Application module for BDAV (aka M2TS) streams.
//!
//! Supports parsing program graphics (PG) and interactive graphics (IG) data.

use super::{
    read_bitfield, AppDetails, MpegTsParser, Packet, Payload, PesUnitObject, Result, SliceReader,
};
use modular_bitfield_msb::prelude::*;

pub mod mobj;
use mobj::MObjCmd;

pub mod pg;
use pg::PgSegmentData;

/// BDAV-specific header prepended to MPEG-TS packets
#[bitfield]
#[derive(Debug)]
pub struct BdavPacketHeader {
    /// Copy protection indicator. Indicates the presence of AACS-protected content.
    pub cpi: B2,
    /// 27 MHz decoder time reference (normally this is not available in *every* MPEG-TS packet).
    pub timestamp: B30,
}

/// Top-level parsed structure for one BDAV packet.
#[derive(Debug)]
pub struct BdavPacket<'a, D> {
    /// BDAV-specific header.
    pub header: BdavPacketHeader,
    /// MPEG-TS packet.
    pub packet: Packet<'a, D>,
}

/// BDAV-specific parsing errors.
#[derive(Debug)]
pub enum BdavErrorDetails {
    /// Encountered an unknown type for [`PgSegmentData`].
    UnknownPgSegmentType(u8),
}

/// Extension trait for parsing BDAV-specific payload data.
pub trait BdavAppDetails: AppDetails<AppErrorDetails = BdavErrorDetails> {}

/// [`BdavAppDetails`] implementation for [`BdavParser::default`].
///
/// Currently just handles parsing [`PgSegmentData`].
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

/// Top-level parser state for 192-byte packets found in BDAV (aka M2TS) streams.
///
/// # Example
///
/// ```no_run
/// use mpegts_io::bdav::BdavParser;
/// use std::fs::File;
/// use std::io::{Read, Result, Seek, SeekFrom};
///
/// fn file_size(file: &mut File) -> Result<u64> {
///     let len = file.seek(SeekFrom::End(0))?;
///     file.seek(SeekFrom::Start(0))?;
///     Ok(len)
/// }
///
/// let mut file = File::open("00000.m2ts").expect("Unable to open!");
/// let num_packets = file_size(&mut file).expect("Unable to get file size!") / 192;
/// let mut parser = BdavParser::default();
/// for _ in 0..num_packets {
///     let mut packet = [0_u8; 192];
///     file.read_exact(&mut packet).expect("IO Error!");
///     let parsed_packet = parser.parse(&packet).expect("Parse Error!");
///     println!("{:?}", parsed_packet);
/// }
/// ```
pub struct BdavParser<D: BdavAppDetails = DefaultBdavAppDetails>(MpegTsParser<D>);

impl Default for BdavParser {
    fn default() -> Self {
        BdavParser::<DefaultBdavAppDetails>(MpegTsParser::default())
    }
}

impl<D: BdavAppDetails> BdavParser<D> {
    /// Parse data for exactly one 192-byte BDAV packet.
    ///
    /// All information about the packet is returned as [`BdavPacket`].
    ///
    /// For payload units that span multiple packets, the relevant pending state is provided in
    /// [`Payload`]. Once the final packet of the unit is read, the entire unit is parsed and made
    /// available in the [`Payload`].
    pub fn parse<'a>(&mut self, packet: &'a [u8; 192]) -> Result<BdavPacket<'a, D>, D> {
        let mut reader = SliceReader::new(packet);
        let header = read_bitfield!(reader, BdavPacketHeader);
        Ok(BdavPacket {
            header,
            packet: self.0.parse_internal(reader)?,
        })
    }
}
