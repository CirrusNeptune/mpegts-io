use super::{MpegTsParser, Payload, Pes, PsiBuilder, Result, SliceReader};
use enum_dispatch::enum_dispatch;
use log::warn;

#[enum_dispatch]
pub(crate) trait PayloadUnitObject {
    fn extend_from_slice(&mut self, slice: &[u8]);
    fn finish<'a>(self, pid: u16, parser: &mut MpegTsParser) -> Result<Payload<'a>>;
    fn pending<'a>(&self) -> Result<Payload<'a>>;
}

#[enum_dispatch(PayloadUnitObject)]
pub(crate) enum PayloadUnit {
    Psi(PsiBuilder),
    Pes(Pes),
}

pub(crate) struct PayloadUnitBuilder {
    unit: PayloadUnit,
    remaining: usize,
}

impl PayloadUnitBuilder {
    pub fn new<T: PayloadUnitObject>(obj: T, obj_length: usize) -> Self
    where
        PayloadUnit: From<T>,
    {
        Self {
            unit: obj.into(),
            remaining: obj_length,
        }
    }

    pub fn append(&mut self, reader: &mut SliceReader) -> Result<bool> {
        if reader.remaining_len() <= self.remaining {
            self.remaining -= reader.remaining_len();
            self.unit.extend_from_slice(reader.read_to_end()?);
            Ok(self.remaining == 0)
        } else {
            self.unit.extend_from_slice(reader.read(self.remaining)?);
            self.remaining = 0;
            Ok(true)
        }
    }

    pub fn finish<'a>(self, pid: u16, parser: &mut MpegTsParser) -> Result<Payload<'a>> {
        assert_eq!(self.remaining, 0);
        self.unit.finish(pid, parser)
    }

    pub fn pending<'a>(&self) -> Result<Payload<'a>> {
        self.unit.pending()
    }
}

impl MpegTsParser {
    pub(crate) fn start_payload_unit<'a, T: PayloadUnitObject>(
        &mut self,
        obj: T,
        length: usize,
        pid: u16,
        reader: &mut SliceReader<'a>,
    ) -> Result<Payload<'a>>
    where
        PayloadUnit: From<T>,
    {
        let mut builder = PayloadUnitBuilder::new(obj, length);
        if builder.append(reader)? {
            builder.finish(pid, self)
        } else {
            let pending = builder.pending();
            self.pending_payload_units.insert(pid, builder);
            pending
        }
    }

    pub(crate) fn continue_payload_unit<'a>(
        &mut self,
        pid: u16,
        reader: &mut SliceReader<'a>,
    ) -> Result<Payload<'a>> {
        match self.pending_payload_units.get_mut(&pid) {
            Some(pes_state) => {
                if pes_state.append(reader)? {
                    self.pending_payload_units
                        .remove(&pid)
                        .unwrap()
                        .finish(pid, self)
                } else {
                    pes_state.pending()
                }
            }
            None => {
                warn!(
                    "Discarding payload unit of unknown continuation PID: {:x}",
                    pid
                );
                Ok(Payload::Unknown)
            }
        }
    }
}
