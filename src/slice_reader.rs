use super::{AppDetails, Error, ErrorDetails, Result};
use std::marker::PhantomData;

/// Simple reader state for extracting data from a [`&[u8]`] slice.
///
/// Unlike the [`std::io::Read`] implementation for [`&[u8]`], this keeps track of the location
/// within the packet for more informative errors via [`Result`].
///
/// # Example
///
/// ```
/// use mpegts_io::SliceReader;
/// let some_data = [0x42];
/// let mut reader = SliceReader::new(&some_data);
/// assert_eq!(reader.read_u8()?, 0x42);
/// # Ok::<(), mpegts_io::Error<mpegts_io::DefaultAppDetails>>(())
/// ```
#[derive(Debug)]
pub struct SliceReader<'a, D> {
    phantom: PhantomData<D>,
    slice: &'a [u8],
    location: usize,
}

impl<'a, D: AppDetails> SliceReader<'a, D> {
    /// Initializes a reader from any byte slice.
    pub fn new(slice: &'a [u8]) -> Self {
        Self {
            phantom: PhantomData,
            slice,
            location: 0,
        }
    }

    /// Creates a fixed `length` sub-reader at the current position, then advances this reader to
    /// the sub-reader's end position.
    ///
    /// The sub-reader semantic makes reading nested data of known lengths easier with correct
    /// bounds checking of the nested data.
    pub fn new_sub_reader(&mut self, length: usize) -> Result<Self, D> {
        let location = self.location;
        Ok(Self {
            phantom: PhantomData,
            slice: self.read(length)?,
            location,
        })
    }

    /// Creates an [`Error`] using the contained location.
    pub fn make_error(&self, details: ErrorDetails<D>) -> Error<D> {
        Error {
            location: self.location,
            details,
        }
    }

    /// Number of bytes remaining in the slice reader.
    pub fn remaining_len(&self) -> usize {
        self.slice.len()
    }

    /// Advance reader without extracting any data from the slice.
    pub fn skip(&mut self, length: usize) -> Result<(), D> {
        if length > self.slice.len() {
            Err(self.make_error(ErrorDetails::<D>::PacketOverrun(length)))
        } else {
            self.location += length;
            self.slice = &self.slice[length..];
            Ok(())
        }
    }

    /// Extract a fixed `length` sub-slice from this reader and advance.
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

    /// Extract a sub-slice of all data remaining to be read.
    pub fn read_to_end(&mut self) -> Result<&'a [u8], D> {
        self.read(self.slice.len())
    }

    /// Same as [`SliceReader::read`] but also converts the slice to an array reference of length
    /// `N`.
    #[allow(unsafe_code)]
    pub fn read_array_ref<const N: usize>(&mut self) -> Result<&'a [u8; N], D> {
        unsafe {
            // Bounds checking performed by read()
            Ok(&*(self.read(N)?.as_ptr() as *const [u8; N]))
        }
    }

    /// Read one byte interpreted as [`u8`].
    pub fn read_u8(&mut self) -> Result<u8, D> {
        Ok(self.read_array_ref::<1>()?[0])
    }

    /// Read two bytes interpreted as big-endian [`u16`].
    pub fn read_be_u16(&mut self) -> Result<u16, D> {
        Ok(u16::from_be_bytes(*self.read_array_ref::<2>()?))
    }

    /// Read three bytes interpreted as big-endian `u24`.
    pub fn read_be_u24(&mut self) -> Result<u32, D> {
        let bytes = *self.read_array_ref::<3>()?;
        Ok(u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]))
    }

    /// Read four bytes interpreted as big-endian [`u32`].
    pub fn read_be_u32(&mut self) -> Result<u32, D> {
        Ok(u32::from_be_bytes(*self.read_array_ref::<4>()?))
    }

    /// Read five bytes interpreted as big-endian `u33`.
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

    /// Extract a fixed `length` sub-slice from this reader without advancing.
    pub fn peek(&mut self, length: usize) -> Result<&'a [u8], D> {
        if length > self.slice.len() {
            Err(self.make_error(ErrorDetails::<D>::PacketOverrun(length)))
        } else {
            Ok(&self.slice[0..length])
        }
    }

    /// Same as [`SliceReader::peek`] but also converts the slice to an array reference of length
    /// `N`.
    #[allow(unsafe_code)]
    pub fn peek_array_ref<const N: usize>(&mut self) -> Result<&'a [u8; N], D> {
        unsafe {
            // Bounds checking performed by read()
            Ok(&*(self.peek(N)?.as_ptr() as *const [u8; N]))
        }
    }
}

/// Convenience macro to read a modular bitfield from a [`SliceReader`]
///
/// Wraps [`SliceReader::read_array_ref`] to read the exact number of bytes required by the
/// bitfield type. Must be expanded in a function that returns [`Result`].
///
/// # Example
///
/// ```
/// use modular_bitfield_msb::prelude::*;
/// use mpegts_io::{read_bitfield, SliceReader};
/// #[bitfield]
/// pub(crate) struct MyBitfield {
///     pub a_bit: B1,
///     #[skip]
///     padding: B7,
/// }
///
/// let some_data = [0x80];
/// let mut reader = SliceReader::new(&some_data);
/// let the_bitfield = read_bitfield!(reader, MyBitfield);
/// assert_eq!(the_bitfield.a_bit(), 1);
/// # Ok::<(), mpegts_io::Error<mpegts_io::DefaultAppDetails>>(())
/// ```
#[macro_export]
macro_rules! read_bitfield {
    ($reader:expr, $type:ty) => {
        <$type>::from_bytes(*$reader.read_array_ref::<{ std::mem::size_of::<$type>() }>()?)
    };
}
