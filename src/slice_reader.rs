use super::{Error, ErrorDetails, Result};

#[derive(Debug)]
pub struct SliceReader<'a> {
    slice: &'a [u8],
    location: usize,
}

impl<'a> SliceReader<'a> {
    pub fn new(slice: &'a [u8]) -> Self {
        Self { slice, location: 0 }
    }

    pub fn new_sub_reader(&mut self, length: usize) -> Result<Self> {
        let location = self.location;
        Ok(Self {
            slice: self.read(length)?,
            location,
        })
    }

    pub fn make_error(&self, details: ErrorDetails) -> Error {
        Error::new(self.location, details)
    }

    pub fn remaining_len(&self) -> usize {
        self.slice.len()
    }

    pub fn skip(&mut self, length: usize) -> Result<()> {
        if length > self.slice.len() {
            Err(self.make_error(ErrorDetails::PacketOverrun(length)))
        } else {
            self.location += length;
            self.slice = &self.slice[length..];
            Ok(())
        }
    }

    pub fn read(&mut self, length: usize) -> Result<&'a [u8]> {
        if length > self.slice.len() {
            Err(self.make_error(ErrorDetails::PacketOverrun(length)))
        } else {
            self.location += length;
            let (left, right) = self.slice.split_at(length);
            self.slice = right;
            Ok(left)
        }
    }

    pub fn read_to_end(&mut self) -> Result<&'a [u8]> {
        self.read(self.slice.len())
    }

    pub fn read_array_ref<const N: usize>(&mut self) -> Result<&'a [u8; N]> {
        unsafe {
            // Bounds checking performed by read()
            Ok(&*(self.read(N)?.as_ptr() as *const [u8; N]))
        }
    }

    pub fn peek(&mut self, length: usize) -> Result<&'a [u8]> {
        if length > self.slice.len() {
            Err(self.make_error(ErrorDetails::PacketOverrun(length)))
        } else {
            Ok(&self.slice[0..length])
        }
    }

    pub fn peek_array_ref<const N: usize>(&mut self) -> Result<&'a [u8; N]> {
        unsafe {
            // Bounds checking performed by read()
            Ok(&*(self.peek(N)?.as_ptr() as *const [u8; N]))
        }
    }
}
