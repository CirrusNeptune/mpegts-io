use super::{Error, ErrorDetails, Result};
use crate::AppDetails;
use std::marker::PhantomData;

#[derive(Debug)]
pub struct SliceReader<'a, D> {
    phantom: PhantomData<D>,
    slice: &'a [u8],
    location: usize,
}

impl<'a, D: AppDetails> SliceReader<'a, D> {
    pub fn new(slice: &'a [u8]) -> Self {
        Self {
            phantom: PhantomData,
            slice,
            location: 0,
        }
    }

    pub fn new_sub_reader(&mut self, length: usize) -> Result<Self, D> {
        let location = self.location;
        Ok(Self {
            phantom: PhantomData,
            slice: self.read(length)?,
            location,
        })
    }

    pub fn make_error(&self, details: ErrorDetails<D>) -> Error<D> {
        Error::new(self.location, details)
    }

    pub fn remaining_len(&self) -> usize {
        self.slice.len()
    }

    pub fn skip(&mut self, length: usize) -> Result<(), D> {
        if length > self.slice.len() {
            Err(self.make_error(ErrorDetails::<D>::PacketOverrun(length)))
        } else {
            self.location += length;
            self.slice = &self.slice[length..];
            Ok(())
        }
    }

    pub fn read(&mut self, length: usize) -> Result<&'a [u8], D> {
        if length > self.slice.len() {
            Err(self.make_error(ErrorDetails::<D>::PacketOverrun(length)))
        } else {
            self.location += length;
            let (left, right) = self.slice.split_at(length);
            self.slice = right;
            Ok(left)
        }
    }

    pub fn read_to_end(&mut self) -> Result<&'a [u8], D> {
        self.read(self.slice.len())
    }

    pub fn read_array_ref<const N: usize>(&mut self) -> Result<&'a [u8; N], D> {
        unsafe {
            // Bounds checking performed by read()
            Ok(&*(self.read(N)?.as_ptr() as *const [u8; N]))
        }
    }

    pub fn read_u8(&mut self) -> Result<u8, D> {
        Ok(self.read_array_ref::<1>()?[0])
    }

    pub fn read_be_u16(&mut self) -> Result<u16, D> {
        Ok(u16::from_be_bytes(*self.read_array_ref::<2>()?))
    }

    pub fn read_be_u24(&mut self) -> Result<u32, D> {
        let bytes = *self.read_array_ref::<3>()?;
        Ok(u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]))
    }

    pub fn read_be_u32(&mut self) -> Result<u32, D> {
        Ok(u32::from_be_bytes(*self.read_array_ref::<4>()?))
    }

    pub fn read_be_u33(&mut self) -> Result<u64, D> {
        let bytes = *self.read_array_ref::<5>()?;
        Ok(u64::from_be_bytes([
            0,
            0,
            0,
            bytes[0] & 0x1,
            bytes[1],
            bytes[2],
            bytes[3],
            bytes[4],
        ]))
    }

    pub fn peek(&mut self, length: usize) -> Result<&'a [u8], D> {
        if length > self.slice.len() {
            Err(self.make_error(ErrorDetails::<D>::PacketOverrun(length)))
        } else {
            Ok(&self.slice[0..length])
        }
    }

    pub fn peek_array_ref<const N: usize>(&mut self) -> Result<&'a [u8; N], D> {
        unsafe {
            // Bounds checking performed by read()
            Ok(&*(self.peek(N)?.as_ptr() as *const [u8; N]))
        }
    }
}

#[macro_export]
macro_rules! read_bitfield {
    ($reader:expr, $type:ty) => {
        <$type>::from_bytes(*$reader.read_array_ref::<{ std::mem::size_of::<$type>() }>()?)
    };
}
