use super::{AppDetails, MpegTsParser, Payload, Pes, PsiBuilder, Result, SliceReader};
use enum_dispatch::enum_dispatch;
use log::warn;

#[enum_dispatch]
pub(crate) trait PayloadUnitObject<D: AppDetails> {
    fn extend_from_slice(&mut self, slice: &[u8]);
    fn finish<'a>(self, pid: u16, parser: &mut MpegTsParser<D>) -> Result<Payload<'a, D>, D>;
    fn pending<'a>(&self) -> Result<Payload<'a, D>, D>;
}

#[enum_dispatch(PayloadUnitObject<D>)]
pub(crate) enum PayloadUnit<D: AppDetails> {
    Psi(PsiBuilder<D>),
    Pes(Pes<D>),
}

pub(crate) struct PayloadUnitBuilder<D: AppDetails> {
    unit: PayloadUnit<D>,
    remaining: usize,
}

impl<D: AppDetails> PayloadUnitBuilder<D> {
    pub fn new<T: PayloadUnitObject<D>>(obj: T, obj_length: usize) -> Self
    where
        PayloadUnit<D>: From<T>,
    {
        Self {
            unit: obj.into(),
            remaining: obj_length,
        }
    }

    pub fn append(&mut self, reader: &mut SliceReader<D>) -> Result<bool, D> {
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

    pub fn finish<'a>(self, pid: u16, parser: &mut MpegTsParser<D>) -> Result<Payload<'a, D>, D> {
        assert_eq!(self.remaining, 0);
        self.unit.finish(pid, parser)
    }

    pub fn pending<'a>(&self) -> Result<Payload<'a, D>, D> {
        self.unit.pending()
    }
}

impl<D: AppDetails> MpegTsParser<D> {
    pub(crate) fn start_payload_unit<'a, T: PayloadUnitObject<D>>(
        &mut self,
        obj: T,
        length: usize,
        pid: u16,
        reader: &mut SliceReader<'a, D>,
    ) -> Result<Payload<'a, D>, D>
    where
        PayloadUnit<D>: From<T>,
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
        reader: &mut SliceReader<'a, D>,
    ) -> Result<Payload<'a, D>, D> {
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
