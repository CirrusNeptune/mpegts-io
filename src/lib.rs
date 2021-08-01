//! Library for reading and writing MPEG transport streams.
//!
//! # Usage
//! Simply add this crate as a dependency in your `Cargo.toml`.
//!
//! ```toml
//! [dependencies]
//! mpegts-io = "~0.1.0"
//! ```

#![allow(unused)]
//#![deny(missing_docs, unsafe_code, warnings)]

use crc::{Crc, Digest, CRC_32_MPEG_2};
use log::warn;
use modular_bitfield_msb::prelude::*;
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};
use std::convert::From;
use std::fmt::{Debug, Formatter};
use std::result;

mod slice_reader;
use slice_reader::SliceReader;

mod span;
use span::{SpanBuilder, SpanObject};

mod psi;
use psi::{PSIBuilder, PSI};

mod pes;
use pes::PES;

mod bdav;
pub use bdav::BDAVParser;

const CRC: Crc<u32> = Crc::<u32>::new(&CRC_32_MPEG_2);
type CrcDigest = Digest<'static, u32>;

#[derive(Debug)]
pub enum ErrorDetails {
    PacketOverrun(usize),
    LostSync,
    BadAdaptationHeader,
    BadPSIHeader,
    BadPESHeader,
    PSICRCMismatch,
}

#[derive(Debug)]
pub struct Error {
    location: usize,
    details: ErrorDetails,
}

impl Error {
    pub fn new(location: usize, details: ErrorDetails) -> Self {
        Self { location, details }
    }
}

pub type Result<T> = result::Result<T, Error>;

#[repr(u8)]
#[derive(Debug, BitfieldSpecifier)]
#[bits = 2]
pub enum TransportScramblingControl {
    NotScrambled,
    Reserved,
    ScrambledEvenKey,
    ScrambledOddKey,
}

#[bitfield]
#[derive(Debug)]
pub struct PacketHeader {
    pub sync_byte: B8,
    pub tei: bool,
    pub pusi: bool,
    pub priority: bool,
    pub pid: B13,
    pub tsc: TransportScramblingControl,
    pub has_adaptation_field: bool,
    pub has_payload: bool,
    pub continuity_counter: B4,
}

#[bitfield]
#[derive(Debug)]
pub struct AdaptationFieldHeader {
    pub length: B8,
    pub discontinuity: bool,
    pub random_access: bool,
    pub priority: bool,
    pub has_pcr: bool,
    pub has_opcr: bool,
    pub has_splice_countdown: bool,
    pub has_transport_private_data: bool,
    pub has_adaptation_field_extension: bool,
}

#[macro_export]
macro_rules! pts_format_args {
    ($pts:expr) => {
        format_args!(
            "{}:{}:{}:{}",
            $pts / (90000 * 60 * 60),
            $pts / (90000 * 60) % 60,
            $pts / 90000 % 60,
            $pts % 90000
        )
    };
}

#[derive(Default, Copy, Clone)]
pub struct PCRTimestamp {
    pub base: u64,
    pub extension: u16,
}

impl Debug for PCRTimestamp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PCRTimestamp")
            .field("base", &pts_format_args!(self.base))
            .field("extension", &self.extension)
            .finish()
    }
}

#[derive(Debug)]
pub struct AdaptationField {
    pub header: AdaptationFieldHeader,
    pub pcr: Option<PCRTimestamp>,
    pub opcr: Option<PCRTimestamp>,
}

#[derive(Debug)]
pub enum Payload<'a> {
    Unknown,
    Raw(SliceReader<'a>),
    PSIPending,
    PSI(PSI),
    PESPending,
    PES(PES),
}

#[derive(Debug)]
pub struct Packet<'a> {
    pub header: PacketHeader,
    pub adaptation_field: Option<AdaptationField>,
    pub payload: Option<Payload<'a>>,
}

#[derive(Default)]
pub struct MPEGTSParser {
    pending_spans: HashMap<u16, SpanBuilder>,
    known_pmt_pids: HashSet<u16>,
    nit_pid: u16,
}

fn is_pes(b: &[u8; 3]) -> bool {
    b[0] == 0 && b[1] == 0 && b[2] == 1
}

fn parse_timestamp(b: &[u8; 5]) -> u64 {
    let mut ts: u64 = ((b[0] & 0x0E) as u64) << 29;
    ts |= (b[1] as u64) << 22;
    ts |= ((b[2] & 0xFE) as u64) << 14;
    ts |= (b[3] as u64) << 7;
    ts |= ((b[4] & 0xFE) as u64) >> 1;
    ts
}

fn parse_pcr(b: &[u8; 6]) -> PCRTimestamp {
    let mut base: u64 = (b[0] as u64) << 25;
    base |= (b[1] as u64) << 17;
    base |= (b[2] as u64) << 9;
    base |= (b[3] as u64) << 1;
    base |= (b[4] as u64) >> 7;

    let mut extension: u16 = ((b[4] & 0x1) as u16) << 8;
    extension |= b[5] as u16;
    PCRTimestamp { base, extension }
}

impl MPEGTSParser {
    fn read_adaptation_field(&mut self, reader: &mut SliceReader) -> Result<AdaptationField> {
        let mut out = AdaptationField {
            header: AdaptationFieldHeader::from_bytes(*reader.read_array_ref::<2>()?),
            pcr: None,
            opcr: None,
        };
        let adaptation_field_length = out.header.length() as usize;
        if adaptation_field_length < 1 || adaptation_field_length > 183 {
            warn!("Bad adaptation field length");
            return Err(reader.make_error(ErrorDetails::BadAdaptationHeader));
        }
        let mut a_reader = reader.new_sub_reader(adaptation_field_length - 1)?;
        if out.header.has_pcr() {
            if a_reader.remaining_len() < 6 {
                warn!("Short read of PCR");
                return Err(reader.make_error(ErrorDetails::BadAdaptationHeader));
            }
            out.pcr = Some(parse_pcr(a_reader.read_array_ref::<6>()?));
        }
        if out.header.has_opcr() {
            if a_reader.remaining_len() < 6 {
                warn!("Short read of OPCR");
                return Err(reader.make_error(ErrorDetails::BadAdaptationHeader));
            }
            out.opcr = Some(parse_pcr(a_reader.read_array_ref::<6>()?));
        }
        // TODO: Splice Countdown
        // TODO: Transport Private Data
        // TODO: Adaptation Extension

        Ok(out)
    }

    fn read_payload<'a>(
        &mut self,
        pusi: bool,
        pid: u16,
        mut reader: SliceReader<'a>,
    ) -> Result<Payload<'a>> {
        if pusi {
            /* Make sure we're not starting an already-started span */
            if self.pending_spans.contains_key(&pid) {
                warn!("Discarding unfinished span packet on PID: {:x}", pid);
                self.pending_spans.remove(&pid);
            }

            /* Check for PAT/PMT/NIT */
            if pid == 0 || self.known_pmt_pids.contains(&pid) || self.nit_pid == pid {
                self.start_psi(pid, &mut reader)
            }
            /* Check for PES if enough payload is present */
            else if reader.remaining_len() >= 6 && is_pes(reader.peek_array_ref::<3>()?) {
                /* PES packet detected */
                self.start_pes(pid, &mut reader)
            } else {
                /* Not enough payload for a PES packet, assume raw */
                Ok(Payload::Raw(reader))
            }
        } else {
            /* Attempt span continuation */
            self.continue_span(pid, &mut reader)
        }
    }

    pub(crate) fn parse_internal<'a>(&mut self, mut reader: SliceReader<'a>) -> Result<Packet<'a>> {
        /* Start with header and verify sync */
        let mut out = Packet {
            header: PacketHeader::from_bytes(*reader.read_array_ref::<4>()?),
            adaptation_field: None,
            payload: None,
        };
        if out.header.sync_byte() != 0x47 {
            return Err(Error::new(0, ErrorDetails::LostSync));
        }

        /* Special cases exist for some PIDs */
        let pid = out.header.pid();

        /* Discard null packets early */
        if pid == 0x1fff {
            return Ok(out);
        }

        /* Read adaptation field if it exists */
        if out.header.has_adaptation_field() {
            out.adaptation_field = Some(self.read_adaptation_field(&mut reader)?);
        }

        /* Read payload if it exists */
        if out.header.has_payload() {
            out.payload = Some(self.read_payload(out.header.pusi(), pid, reader)?);
        }

        Ok(out)
    }

    pub fn parse<'a>(&mut self, packet: &'a [u8; 188]) -> Result<Packet<'a>> {
        let reader = SliceReader::new(packet);
        self.parse_internal(reader)
    }
}