//! PNG in and out: an 8-bit writer for headless screenshots
//! ([`encode`]/[`write`]), and a decoder ([`decode`]) for the PNGs a real
//! icon theme ships. See `decode`'s module docs for exactly what is and is
//! not read.

mod checksum;
mod decode;
mod encode;
mod filter;
mod inflate;

pub use decode::{decode, DecodeError};
pub use encode::{encode, write};
