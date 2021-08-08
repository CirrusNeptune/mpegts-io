use super::{
    read_bitfield, AppDetails, CrcDigest, Error, ErrorDetails, MpegTsParser, Payload,
    PayloadUnitObject, Result, SliceReader, CRC,
};
use log::warn;
use modular_bitfield_msb::prelude::*;
use smallvec::SmallVec;
use std::marker::PhantomData;

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
    pub fn new_from_reader<D: AppDetails>(reader: &mut SliceReader<D>) -> Result<Self, D> {
        let tag = reader.read_u8()?;
        let len = reader.read_u8()?;
        let mut data = SmallVec::<[u8; 8]>::new();
        data.extend_from_slice(reader.read(len as usize)?);
        Ok(Self { tag, data })
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
}

#[derive(Debug)]
pub struct Psi {
    pub header: PsiHeader,
    pub table_syntax: Option<PsiTableSyntax>,
    pub data: PsiData,
}

pub struct PsiBuilder<D> {
    phantom: PhantomData<D>,
    header: PsiHeader,
    table_syntax: Option<PsiTableSyntax>,
    data: Vec<u8>,
    hasher: Option<CrcDigest>,
}

impl<D: AppDetails> PsiBuilder<D> {
    pub fn new(
        capacity: usize,
        header: PsiHeader,
        table_syntax: Option<PsiTableSyntax>,
        hasher: CrcDigest,
    ) -> Self {
        Self {
            phantom: PhantomData,
            header,
            table_syntax,
            data: Vec::with_capacity(capacity),
            hasher: Some(hasher),
        }
    }

    fn finish_substitute_data<'a>(mut self, data: PsiData) -> Result<Payload<'a, D>, D> {
        Ok(Payload::Psi(Psi {
            header: self.header,
            table_syntax: self.table_syntax,
            data,
        }))
    }

    fn finish_keep_raw_data<'a>(mut self) -> Result<Payload<'a, D>, D> {
        Ok(Payload::Psi(Psi {
            header: self.header,
            table_syntax: self.table_syntax,
            data: PsiData::Raw(self.data),
        }))
    }

    fn finish_pat<'a>(mut self, parser: &mut MpegTsParser<D>) -> Result<Payload<'a, D>, D> {
        parser.known_pmt_pids.clear();
        let mut reader = SliceReader::new(self.data.as_slice());
        let mut pat_vec = Vec::with_capacity(reader.remaining_len() / 4);
        while reader.remaining_len() >= 4 {
            let entry = read_bitfield!(reader, PatEntry);
            parser.known_pmt_pids.insert(entry.program_map_pid());
            pat_vec.push(entry);
        }
        self.finish_substitute_data(PsiData::Pat(pat_vec))
    }

    fn finish_pmt<'a>(mut self, parser: &mut MpegTsParser<D>) -> Result<Payload<'a, D>, D> {
        let mut reader = SliceReader::new(self.data.as_slice());
        let header = read_bitfield!(reader, PmtHeader);
        let mut pmt = Pmt {
            header,
            program_descriptors: Vec::new(),
            es_infos: Vec::new(),
        };
        let mut info_reader = reader.new_sub_reader(pmt.header.program_info_length() as usize)?;
        while info_reader.remaining_len() > 0 {
            let descriptor = Descriptor::new_from_reader(&mut info_reader)?;
            pmt.program_descriptors.push(descriptor);
        }
        while reader.remaining_len() > 0 {
            let es_header = read_bitfield!(reader, ElementaryStreamInfoHeader);
            let mut es_info = ElementaryStreamInfo {
                header: es_header,
                es_descriptors: SmallVec::new(),
            };
            let mut es_reader = reader.new_sub_reader(es_info.header.es_info_length() as usize)?;
            while es_reader.remaining_len() > 0 {
                let descriptor = Descriptor::new_from_reader(&mut es_reader)?;
                es_info.es_descriptors.push(descriptor);
            }
            pmt.es_infos.push(es_info);
        }
        self.finish_substitute_data(PsiData::Pmt(pmt))
    }
}

impl<D: AppDetails> PayloadUnitObject<D> for PsiBuilder<D> {
    fn extend_from_slice(&mut self, slice: &[u8]) {
        self.data.extend_from_slice(slice);
    }

    fn finish<'a>(mut self, pid: u16, parser: &mut MpegTsParser<D>) -> Result<Payload<'a, D>, D> {
        /* Validate using CRC32 */
        let len_minus_crc = self.data.len() - 4;
        let mut hasher = self.hasher.take().expect("PSI hasher not set");
        hasher.update(&self.data[..len_minus_crc]);
        let actual_hash = hasher.finalize();
        let expected_hash = SliceReader::new(&self.data[len_minus_crc..]).read_be_u32()?;
        if expected_hash != actual_hash {
            warn!("PSI hash mismatch for PID: {:x}", pid);
            return Err(Error::new(0, ErrorDetails::<D>::PsiCrcMismatch));
        }
        self.data.truncate(len_minus_crc);

        /* Process table based on known type */
        if self.header.private_bit() {
            /* Private tables are not defined in ISO/IEC 13818-1 */
            self.finish_keep_raw_data()
        } else if pid == 0 && self.header.table_id() == 0 {
            /* PAT */
            self.finish_pat(parser)
        } else if parser.known_pmt_pids.contains(&pid) {
            /* PMT */
            self.finish_pmt(parser)
        } else {
            /* Unhandled table type (CAT?); keep data raw */
            self.finish_keep_raw_data()
        }
    }

    fn pending<'a>(&self) -> Result<Payload<'a, D>, D> {
        Ok(Payload::PsiPending)
    }
}

impl<D: AppDetails> MpegTsParser<D> {
    pub(crate) fn start_psi<'a>(
        &mut self,
        pid: u16,
        reader: &mut SliceReader<'a, D>,
    ) -> Result<Payload<'a, D>, D> {
        if reader.remaining_len() < 1 {
            warn!("Short read of PSI pointer field");
            return Err(reader.make_error(ErrorDetails::<D>::BadPsiHeader));
        }
        let pointer_field = reader.read(1)?[0];
        if reader.remaining_len() < pointer_field as usize {
            warn!("Short read of PSI pointer filler");
            return Err(reader.make_error(ErrorDetails::<D>::BadPsiHeader));
        }
        reader.skip(pointer_field as usize)?;

        if reader.remaining_len() < 3 {
            warn!("Short read of PSI header");
            return Err(reader.make_error(ErrorDetails::<D>::BadPsiHeader));
        }
        let mut hasher = CRC.digest();
        let psi_header_bytes = reader.read_array_ref::<3>()?;
        hasher.update(psi_header_bytes);
        let psi_header = PsiHeader::from_bytes(*psi_header_bytes);
        let section_length = psi_header.section_length();

        if section_length > 0 {
            if reader.remaining_len() < 5 {
                warn!("Short read of PSI table syntax");
                return Err(reader.make_error(ErrorDetails::<D>::BadPsiHeader));
            }
            let psi_table_syntax_bytes = reader.read_array_ref::<5>()?;
            hasher.update(psi_table_syntax_bytes);
            let psi_table_syntax = PsiTableSyntax::from_bytes(*psi_table_syntax_bytes);

            let table_length = (section_length - 5) as usize;
            if table_length < 4 {
                /* Must have length to read at least the CRC32 */
                warn!("Insufficient table length");
                return Err(reader.make_error(ErrorDetails::<D>::BadPsiHeader));
            }

            self.start_payload_unit(
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
