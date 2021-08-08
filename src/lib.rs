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
use std::ops::Range;
use std::rc::Rc;
use std::result;

mod slice_reader;
use slice_reader::SliceReader;

mod payload_unit;
use payload_unit::{PayloadUnitBuilder, PayloadUnitObject};

mod psi;
use psi::{Psi, PsiBuilder};

mod pes;
use pes::{Pes, PesUnitObject, PesUnitObjectFactory};

mod bdav;
pub use bdav::{BdavParser, DefaultBdavAppDetails};

const CRC: Crc<u32> = Crc::<u32>::new(&CRC_32_MPEG_2);
type CrcDigest = Digest<'static, u32>;

#[derive(Debug)]
pub enum ErrorDetails<D: AppDetails> {
    PacketOverrun(usize),
    LostSync,
    BadAdaptationHeader,
    BadPsiHeader,
    BadPesHeader,
    PsiCrcMismatch,
    AppError(D::AppErrorDetails),
}

pub trait AppDetails: Default {
    type AppErrorDetails: Debug;
    fn new_pes_unit_data(pid: u16, unit_length: usize) -> Option<Box<dyn PesUnitObject<Self>>>;
}

#[derive(Default)]
pub struct DefaultAppDetails;

impl AppDetails for DefaultAppDetails {
    type AppErrorDetails = ();

    fn new_pes_unit_data(pid: u16, unit_length: usize) -> Option<Box<dyn PesUnitObject<Self>>> {
        None
    }
}

#[derive(Debug)]
pub struct Error<D: AppDetails> {
    location: usize,
    details: ErrorDetails<D>,
}

impl<D: AppDetails> Error<D> {
    pub fn new(location: usize, details: ErrorDetails<D>) -> Self {
        Self { location, details }
    }
}

pub type Result<T, D> = result::Result<T, Error<D>>;

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
pub struct PcrTimestamp {
    pub base: u64,
    pub extension: u16,
}

impl Debug for PcrTimestamp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PcrTimestamp")
            .field("base", &pts_format_args!(self.base))
            .field("extension", &self.extension)
            .finish()
    }
}

#[derive(Debug)]
pub struct AdaptationField {
    pub header: AdaptationFieldHeader,
    pub pcr: Option<PcrTimestamp>,
    pub opcr: Option<PcrTimestamp>,
}

#[derive(Debug)]
pub enum Payload<'a, D> {
    Unknown,
    Raw(SliceReader<'a, D>),
    PsiPending,
    Psi(Psi),
    PesPending,
    Pes(Pes<D>),
}

#[derive(Debug)]
pub struct Packet<'a, D> {
    pub header: PacketHeader,
    pub adaptation_field: Option<AdaptationField>,
    pub payload: Option<Payload<'a, D>>,
}

#[derive(Default)]
pub struct MpegTsParser<D: AppDetails = DefaultAppDetails> {
    pending_payload_units: HashMap<u16, PayloadUnitBuilder<D>>,
    known_pmt_pids: HashSet<u16>,
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

fn parse_pcr(b: &[u8; 6]) -> PcrTimestamp {
    let mut base: u64 = (b[0] as u64) << 25;
    base |= (b[1] as u64) << 17;
    base |= (b[2] as u64) << 9;
    base |= (b[3] as u64) << 1;
    base |= (b[4] as u64) >> 7;

    let mut extension: u16 = ((b[4] & 0x1) as u16) << 8;
    extension |= b[5] as u16;
    PcrTimestamp { base, extension }
}

impl<D: AppDetails> MpegTsParser<D> {
    fn read_adaptation_field(&mut self, reader: &mut SliceReader<D>) -> Result<AdaptationField, D> {
        let mut out = AdaptationField {
            header: read_bitfield!(reader, AdaptationFieldHeader),
            pcr: None,
            opcr: None,
        };
        let adaptation_field_length = out.header.length() as usize;
        if !(1..=183).contains(&adaptation_field_length) {
            warn!("Bad adaptation field length");
            return Err(reader.make_error(ErrorDetails::<D>::BadAdaptationHeader));
        }
        let mut a_reader = reader.new_sub_reader(adaptation_field_length - 1)?;
        if out.header.has_pcr() {
            if a_reader.remaining_len() < 6 {
                warn!("Short read of PCR");
                return Err(reader.make_error(ErrorDetails::<D>::BadAdaptationHeader));
            }
            out.pcr = Some(parse_pcr(a_reader.read_array_ref::<6>()?));
        }
        if out.header.has_opcr() {
            if a_reader.remaining_len() < 6 {
                warn!("Short read of OPCR");
                return Err(reader.make_error(ErrorDetails::<D>::BadAdaptationHeader));
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
        mut reader: SliceReader<'a, D>,
    ) -> Result<Payload<'a, D>, D> {
        if pusi {
            /* Make sure we're not starting an already-started unit */
            if self.pending_payload_units.contains_key(&pid) {
                warn!("Discarding unfinished unit packet on PID: {:x}", pid);
                self.pending_payload_units.remove(&pid);
            }

            /* Check for PAT/PMT/NIT */
            if pid == 0 || self.known_pmt_pids.contains(&pid) {
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
            /* Attempt unit continuation */
            self.continue_payload_unit(pid, &mut reader)
        }
    }

    pub(crate) fn parse_internal<'a>(
        &mut self,
        mut reader: SliceReader<'a, D>,
    ) -> Result<Packet<'a, D>, D> {
        /* Start with header and verify sync */
        let mut out = Packet {
            header: read_bitfield!(reader, PacketHeader),
            adaptation_field: None,
            payload: None,
        };
        if out.header.sync_byte() != 0x47 {
            return Err(reader.make_error(ErrorDetails::<D>::LostSync));
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

    pub fn parse<'a>(&mut self, packet: &'a [u8; 188]) -> Result<Packet<'a, D>, D> {
        let reader = SliceReader::new(packet);
        self.parse_internal(reader)
    }
}
