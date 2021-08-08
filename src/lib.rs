//! Library for reading and (eventually) writing MPEG transport streams.
//!
//! # Usage
//! Simply add this crate as a dependency in your `Cargo.toml`.
//!
//! ```toml
//! [dependencies]
//! mpegts-io = "~0.1.0"
//! ```

#![allow(unused)]
#![deny(missing_docs, unsafe_code, warnings)]

use crc::{Crc, Digest, CRC_32_MPEG_2};
use log::warn;
use modular_bitfield_msb::prelude::*;
use std::collections::{HashMap, HashSet};
use std::convert::From;
use std::fmt::{Debug, Formatter};
use std::result;

mod slice_reader;
pub use slice_reader::SliceReader;

mod payload_unit;
use payload_unit::{PayloadUnitBuilder, PayloadUnitObject};

mod psi;
use psi::PsiBuilder;
pub use psi::{
    Descriptor, ElementaryStreamInfo, ElementaryStreamInfoHeader, PatEntry, PmtHeader, Psi,
    PsiData, PsiHeader, PsiTableSyntax,
};

mod pes;
pub use pes::{Pes, PesHeader, PesOptionalHeader, PesUnitObject};

pub mod bdav;
use bdav::DefaultBdavAppDetails;

const CRC: Crc<u32> = Crc::<u32>::new(&CRC_32_MPEG_2);
type CrcDigest = Digest<'static, u32>;

/// Errors that may be encountered while parsing an MPEG transport stream.
///
/// Contains built-in errors for packet headers, PSI, and PES parsing. Applications may extend this
/// for their own payload parsers via [`AppDetails::AppErrorDetails`] in the
/// [`ErrorDetails::AppError`] variant.
#[derive(Debug)]
pub enum ErrorDetails<D: AppDetails> {
    /// Encountered when a [`SliceReader`] reads out of bounds.
    /// The [`usize`] parameter is the length of the offending read.
    PacketOverrun(usize),
    /// MPEG-TS packet headers must contain a sync byte of 0x47.
    /// This is the error when encountering any other value.
    LostSync,
    /// Encountered for inconsistent [`AdaptationFieldHeader`] parses.
    BadAdaptationHeader,
    /// Encountered for inconsistent [`PsiHeader`] parses.
    BadPsiHeader,
    /// Encountered for inconsistent [`PesHeader`] or [`PesOptionalHeader`] parses.
    BadPesHeader,
    /// Encountered when a PSI unit fails CRC check.
    PsiCrcMismatch,
    /// Application-defined error extension. Specified via [`AppDetails::AppErrorDetails`].
    AppError(D::AppErrorDetails),
}

/// Allows the application to extend the parser with PES payload parsers ([`PesUnitObject`])
/// and an error extension variant for these parsers via [`ErrorDetails::AppError`].
///
/// See [`DefaultBdavAppDetails`] for an example of an application-defined AppDetails.
pub trait AppDetails: Default {
    /// The extension error type exposed via [`ErrorDetails::AppError`].
    type AppErrorDetails: Debug;

    /// Application-defined function to map a PES unit-start packet's `pid` into a new
    /// [`PesUnitObject`].
    ///
    /// The finished object will be returned to the application via [`Payload::Pes`] when the final
    /// packet is read.
    fn new_pes_unit_data(pid: u16, unit_length: usize) -> Option<Box<dyn PesUnitObject<Self>>>;
}

/// Basic [`AppDetails`] implementation with no added functionality.
#[derive(Default, Debug)]
pub struct DefaultAppDetails;

impl AppDetails for DefaultAppDetails {
    type AppErrorDetails = ();

    fn new_pes_unit_data(pid: u16, unit_length: usize) -> Option<Box<dyn PesUnitObject<Self>>> {
        None
    }
}

/// Error type encapsulating all possible parser errors.
#[derive(Debug)]
pub struct Error<D: AppDetails> {
    /// Byte index within the packet that the error was encountered.
    pub location: usize,
    /// Information about the error.
    pub details: ErrorDetails<D>,
}

/// [`std::result::Result`] alias that uses [`Error`].
pub type Result<T, D> = result::Result<T, Error<D>>;

/// TSC information used in a packet's payload.
#[repr(u8)]
#[derive(Debug, BitfieldSpecifier)]
#[bits = 2]
pub enum TransportScramblingControl {
    /// Not scrambled.
    NotScrambled,
    /// Do not use.
    Reserved,
    /// Scrambled with even key.
    ScrambledEvenKey,
    /// Scrambled with odd key.
    ScrambledOddKey,
}

/// Link-layer header found at the start of every 188-byte MPEG-TS packet.
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

/// Packets may contain adaptation meta data in addition or in lieu of payload data. This header
/// specifies the particular type(s) of meta-data contained.
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

/// Expands to [`format_args`] for a 90kHz timestamp of any integer type.
///
/// Format is <hours>:<minutes>:<seconds>:<90kHz-ticks>
///
/// # Example
///
/// ```
/// use mpegts_io::pts_format_args;
/// assert_eq!(std::fmt::format(pts_format_args!(900000)), "0:0:10:0");
/// ```
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

/// Program clock reference (PCR) for synchronizing the decoder with the encoder.
///
/// Periodically sent for every program contained in the transport stream.
#[derive(Default, Copy, Clone)]
pub struct PcrTimestamp {
    /// 33-bits of a 90kHz base clock. May be formatted with [`pts_format_args`].
    pub base: u64,
    /// 9-bits of a 27MHz clock rolling over every 300 counts to the base.
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

/// Non-payload packet metadata.
#[derive(Debug)]
pub struct AdaptationField {
    /// Header describing which fields are contained.
    pub header: AdaptationFieldHeader,
    /// Program Clock Reference.
    pub pcr: Option<PcrTimestamp>,
    /// Original Program Clock Reference.
    pub opcr: Option<PcrTimestamp>,
}

/// Parsed payload of the packet.
///
/// If the packet is part of an incomplete payload unit, the appropriate pending variant is set.
#[derive(Debug)]
pub enum Payload<'a, D> {
    /// Unhandled payload type; parsing is left to the application.
    Raw(SliceReader<'a, D>),
    /// PSI payload unit is incomplete.
    PsiPending,
    /// Complete parsed PSI payload.
    Psi(Psi),
    /// PES payload unit is incomplete.
    PesPending,
    /// Complete parsed PES payload.
    Pes(Pes<D>),
}

/// Top-level parsed structure for one MPEG-TS packet.
#[derive(Debug)]
pub struct Packet<'a, D> {
    /// Packet link-layer header.
    pub header: PacketHeader,
    /// Optional adaptation field metadata.
    pub adaptation_field: Option<AdaptationField>,
    /// Optional payload data.
    pub payload: Option<Payload<'a, D>>,
}

/// MPEG-TS parser state capable of assembling payload units.
///
/// # Example
///
/// ```no_run
/// use mpegts_io::{DefaultAppDetails, MpegTsParser};
/// use std::fs::File;
/// use std::io::{Read, Result, Seek, SeekFrom};
///
/// fn file_size(file: &mut File) -> Result<u64> {
///     let len = file.seek(SeekFrom::End(0))?;
///     file.seek(SeekFrom::Start(0))?;
///     Ok(len)
/// }
///
/// let mut file = File::open("00000.ts").expect("Unable to open!");
/// let num_packets = file_size(&mut file).expect("Unable to get file size!") / 188;
/// let mut parser = MpegTsParser::<DefaultAppDetails>::default();
/// for _ in 0..num_packets {
///     let mut packet = [0_u8; 188];
///     file.read_exact(&mut packet).expect("IO Error!");
///     let parsed_packet = parser.parse(&packet).expect("Parse Error!");
///     println!("{:?}", parsed_packet);
/// }
/// ```
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
            self.continue_payload_unit(pid, reader)
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

    /// Parse data for exactly one 188-byte MPEG-TS packet.
    ///
    /// All information about the packet is returned as [`Packet`].
    ///
    /// For payload units that span multiple packets, the relevant pending state is provided in
    /// [`Payload`]. Once the final packet of the unit is read, the entire unit is parsed and made
    /// available in the [`Payload`].
    pub fn parse<'a>(&mut self, packet: &'a [u8; 188]) -> Result<Packet<'a, D>, D> {
        let reader = SliceReader::new(packet);
        self.parse_internal(reader)
    }
}
