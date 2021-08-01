use super::{
    CrcDigest, Error, ErrorDetails, MpegTsParser, Payload, Result, SliceReader, SpanObject, CRC,
};
use log::warn;
use modular_bitfield_msb::prelude::*;
use smallvec::SmallVec;

#[bitfield]
#[derive(Debug)]
pub struct PsiHeader {
    pub table_id: B8,
    pub section_syntax_indicator: bool,
    pub private_bit: bool,
    pub reserved_bits: B2,
    #[skip]
    pub unused_bits: B2,
    pub section_length: B10,
}

#[bitfield]
#[derive(Debug)]
pub struct PsiTableSyntax {
    pub table_id_extension: B16,
    pub reserved_bits: B2,
    pub version: B5,
    pub current_next_indicator: bool,
    pub section_num: B8,
    pub last_section_num: B8,
}

#[bitfield]
#[derive(Debug)]
pub struct PatEntry {
    pub program_num: B16,
    pub reserved: B3,
    pub program_map_pid: B13,
}

#[derive(Debug)]
pub struct Descriptor {
    pub tag: u8,
    pub data: SmallVec<[u8; 8]>,
}

impl Descriptor {
    pub fn new_from_reader(reader: &mut SliceReader) -> Result<Self> {
        let tag_len = reader.read_array_ref::<2>()?;
        let mut data = SmallVec::<[u8; 8]>::new();
        data.extend_from_slice(reader.read(tag_len[1] as usize)?);
        Ok(Self {
            tag: tag_len[0],
            data,
        })
    }
}

#[bitfield]
#[derive(Debug)]
pub struct PmtHeader {
    pub reserved: B3,
    pub pcr_pid: B13,
    pub reserved2: B4,
    #[skip]
    pub unused_bits: B2,
    pub program_info_length: B10,
}

#[bitfield]
#[derive(Debug)]
pub struct ElementaryStreamInfoHeader {
    pub stream_type: B8,
    pub reserved: B3,
    pub elementary_pid: B13,
    pub reserved2: B4,
    #[skip]
    pub unused_bits: B2,
    pub es_info_length: B10,
}

#[derive(Debug)]
pub struct ElementaryStreamInfo {
    pub header: ElementaryStreamInfoHeader,
    pub es_descriptors: SmallVec<[Descriptor; 4]>,
}

#[derive(Debug)]
pub struct Pmt {
    pub header: PmtHeader,
    pub program_descriptors: Vec<Descriptor>,
    pub es_infos: Vec<ElementaryStreamInfo>,
}

#[derive(Debug)]
pub enum PsiData {
    Raw(Vec<u8>),
    Pat(Vec<PatEntry>),
    Pmt(Pmt),
    Nit(Vec<u8>),
}

#[derive(Debug)]
pub struct Psi {
    pub header: PsiHeader,
    pub table_syntax: Option<PsiTableSyntax>,
    pub data: PsiData,
}

pub struct PsiBuilder {
    header: PsiHeader,
    table_syntax: Option<PsiTableSyntax>,
    data: Vec<u8>,
    hasher: CrcDigest,
}

impl PsiBuilder {
    pub fn new(
        capacity: usize,
        header: PsiHeader,
        table_syntax: Option<PsiTableSyntax>,
        hasher: CrcDigest,
    ) -> Self {
        Self {
            header,
            table_syntax,
            data: Vec::with_capacity(capacity),
            hasher,
        }
    }
}

impl SpanObject for PsiBuilder {
    fn extend_from_slice(&mut self, slice: &[u8]) {
        self.data.extend_from_slice(slice);
    }

    fn finish<'a>(mut self, pid: u16, parser: &mut MpegTsParser) -> Result<Payload<'a>> {
        /* Validate using CRC32 */
        let len_minus_crc = self.data.len() - 4;
        self.hasher.update(&self.data[..len_minus_crc]);
        let actual_hash = self.hasher.finalize();
        let expected_hash = u32::from_be_bytes(
            *SliceReader::new(&self.data[len_minus_crc..])
                .read_array_ref::<4>()
                .unwrap(),
        );
        if expected_hash != actual_hash {
            warn!("PSI hash mismatch for PID: {:x}", pid);
            return Err(Error::new(0, ErrorDetails::PSICRCMismatch));
        }
        self.data.truncate(len_minus_crc);

        /* Process table based on known type */
        if pid == 0 && self.header.table_id() == 0 {
            /* PAT */
            parser.nit_pid = 0x0010;
            parser.known_pmt_pids.clear();
            let mut reader = SliceReader::new(self.data.as_slice());
            let mut pat_vec = Vec::with_capacity(reader.remaining_len() / 4);
            while reader.remaining_len() >= 4 {
                let entry = PatEntry::from_bytes(*reader.read_array_ref::<4>().unwrap());
                if entry.program_num() == 0 {
                    parser.nit_pid = entry.program_map_pid();
                } else {
                    parser.known_pmt_pids.insert(entry.program_map_pid());
                }
                pat_vec.push(entry);
            }
            Ok(Payload::PSI(Psi {
                header: self.header,
                table_syntax: self.table_syntax,
                data: PsiData::Pat(pat_vec),
            }))
        } else if parser.nit_pid == pid {
            /* NIT */
            Ok(Payload::PSI(Psi {
                header: self.header,
                table_syntax: self.table_syntax,
                data: PsiData::Nit(self.data),
            }))
        } else if parser.known_pmt_pids.contains(&pid) {
            /* PMT */
            let mut reader = SliceReader::new(self.data.as_slice());
            let header = PmtHeader::from_bytes(*reader.read_array_ref::<4>()?);
            let mut pmt = Pmt {
                header,
                program_descriptors: Vec::new(),
                es_infos: Vec::new(),
            };
            let mut info_reader =
                reader.new_sub_reader(pmt.header.program_info_length() as usize)?;
            while info_reader.remaining_len() > 0 {
                let descriptor = Descriptor::new_from_reader(&mut info_reader)?;
                pmt.program_descriptors.push(descriptor);
            }
            while reader.remaining_len() > 0 {
                let es_header =
                    ElementaryStreamInfoHeader::from_bytes(*reader.read_array_ref::<5>()?);
                let mut es_info = ElementaryStreamInfo {
                    header: es_header,
                    es_descriptors: SmallVec::new(),
                };
                let mut es_reader =
                    reader.new_sub_reader(es_info.header.es_info_length() as usize)?;
                while es_reader.remaining_len() > 0 {
                    let descriptor = Descriptor::new_from_reader(&mut es_reader)?;
                    es_info.es_descriptors.push(descriptor);
                }
                pmt.es_infos.push(es_info);
            }
            Ok(Payload::PSI(Psi {
                header: self.header,
                table_syntax: self.table_syntax,
                data: PsiData::Pmt(pmt),
            }))
        } else {
            /* Unhandled table type; keep data raw */
            Ok(Payload::PSI(Psi {
                header: self.header,
                table_syntax: self.table_syntax,
                data: PsiData::Raw(self.data),
            }))
        }
    }

    fn pending<'a>(&self) -> Result<Payload<'a>> {
        Ok(Payload::PSIPending)
    }
}

impl MpegTsParser {
    pub(crate) fn start_psi<'a>(
        &mut self,
        pid: u16,
        reader: &mut SliceReader<'a>,
    ) -> Result<Payload<'a>> {
        if reader.remaining_len() < 1 {
            warn!("Short read of PSI pointer field");
            return Err(reader.make_error(ErrorDetails::BadPSIHeader));
        }
        let pointer_field = reader.read(1)?[0];
        if reader.remaining_len() < pointer_field as usize {
            warn!("Short read of PSI pointer filler");
            return Err(reader.make_error(ErrorDetails::BadPSIHeader));
        }
        reader.skip(pointer_field as usize)?;

        if reader.remaining_len() < 3 {
            warn!("Short read of PSI header");
            return Err(reader.make_error(ErrorDetails::BadPSIHeader));
        }
        let mut hasher = CRC.digest();
        let psi_header_bytes = reader.read_array_ref::<3>()?;
        hasher.update(psi_header_bytes);
        let psi_header = PsiHeader::from_bytes(*psi_header_bytes);
        let section_length = psi_header.section_length();

        if section_length > 0 {
            if reader.remaining_len() < 5 {
                warn!("Short read of PSI table syntax");
                return Err(reader.make_error(ErrorDetails::BadPSIHeader));
            }
            let psi_table_syntax_bytes = reader.read_array_ref::<5>()?;
            hasher.update(psi_table_syntax_bytes);
            let psi_table_syntax = PsiTableSyntax::from_bytes(*psi_table_syntax_bytes);

            let table_length = (section_length - 5) as usize;
            if table_length < 4 {
                /* Must have length to read at least the CRC32 */
                warn!("Insufficient table length");
                return Err(reader.make_error(ErrorDetails::BadPSIHeader));
            }

            self.start_span(
                PsiBuilder::new(table_length, psi_header, Some(psi_table_syntax), hasher),
                table_length,
                pid,
                reader,
            )
        } else {
            PsiBuilder::new(0, psi_header, None, hasher).finish(pid, self)
        }
    }
}
