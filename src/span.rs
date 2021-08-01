use super::{MpegTsParser, Payload, Pes, PsiBuilder, Result, SliceReader};
use enum_dispatch::enum_dispatch;
use log::warn;

#[enum_dispatch]
pub(crate) trait SpanObject {
    fn extend_from_slice(&mut self, slice: &[u8]);
    fn finish<'a>(self, pid: u16, parser: &mut MpegTsParser) -> Result<Payload<'a>>;
    fn pending<'a>(&self) -> Result<Payload<'a>>;
}

#[enum_dispatch(SpanObject)]
pub(crate) enum Span {
    Psi(PsiBuilder),
    Pes(Pes),
}

pub(crate) struct SpanBuilder {
    span: Span,
    remaining: usize,
}

impl SpanBuilder {
    pub fn new<T: SpanObject>(obj: T, obj_length: usize) -> Self
    where
        Span: From<T>,
    {
        Self {
            span: obj.into(),
            remaining: obj_length,
        }
    }

    pub fn append(&mut self, reader: &mut SliceReader) -> Result<bool> {
        if reader.remaining_len() <= self.remaining {
            self.remaining -= reader.remaining_len();
            self.span.extend_from_slice(reader.read_to_end()?);
            Ok(self.remaining == 0)
        } else {
            self.span.extend_from_slice(reader.read(self.remaining)?);
            self.remaining = 0;
            Ok(true)
        }
    }

    pub fn finish<'a>(self, pid: u16, parser: &mut MpegTsParser) -> Result<Payload<'a>> {
        assert_eq!(self.remaining, 0);
        self.span.finish(pid, parser)
    }

    pub fn pending<'a>(&self) -> Result<Payload<'a>> {
        self.span.pending()
    }
}

impl MpegTsParser {
    pub(crate) fn start_span<'a, T: SpanObject>(
        &mut self,
        obj: T,
        length: usize,
        pid: u16,
        reader: &mut SliceReader<'a>,
    ) -> Result<Payload<'a>>
    where
        Span: From<T>,
    {
        let mut span_builder = SpanBuilder::new(obj, length);
        if span_builder.append(reader)? {
            span_builder.finish(pid, self)
        } else {
            let pending = span_builder.pending();
            self.pending_spans.insert(pid, span_builder);
            pending
        }
    }

    pub(crate) fn continue_span<'a>(
        &mut self,
        pid: u16,
        reader: &mut SliceReader<'a>,
    ) -> Result<Payload<'a>> {
        match self.pending_spans.get_mut(&pid) {
            Some(pes_state) => {
                if pes_state.append(reader)? {
                    self.pending_spans.remove(&pid).unwrap().finish(pid, self)
                } else {
                    pes_state.pending()
                }
            }
            None => {
                warn!(
                    "Discarding payload of unknown span continuation PID: {:x}",
                    pid
                );
                Ok(Payload::Unknown)
            }
        }
    }
}
