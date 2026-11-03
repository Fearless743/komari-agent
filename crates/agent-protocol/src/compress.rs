//! gzip transport helpers for protocol v2.
//!
//! The Go agent compresses v2 POST bodies with `compress/gzip` at
//! `DefaultCompression` (level 6) and sends `Content-Encoding: gzip`. The same
//! level is used here. The header is pinned to `mtime = 0` and
//! `operating_system = 255` ("unknown"), which is what Go's `gzip.Writer`
//! emits, so both implementations produce byte-identical headers.

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use flate2::GzBuilder;

use crate::ProtocolError;

/// Compression level used by Go's `gzip.NewWriter` (`DefaultCompression`).
pub const GZIP_DEFAULT_LEVEL: u32 = 6;

/// Value Go writes into the gzip OS header byte ("unknown").
pub const GZIP_OS_UNKNOWN: u8 = 255;

/// Compress with the Go-compatible default level (6).
///
/// Compression is deterministic: the gzip header has no timestamp and no file
/// name, so identical input always yields identical output.
pub fn compress(data: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    compress_with_level(data, GZIP_DEFAULT_LEVEL)
}

/// Compress with an explicit level in `0..=9` (`0` = store, `9` = best).
///
/// Go additionally supports `-1` (default) and `-2` (Huffman only); use
/// [`compress`] for the default and [`compress_huffman_only`] for the latter.
pub fn compress_with_level(data: &[u8], level: u32) -> Result<Vec<u8>, ProtocolError> {
    if level > 9 {
        return Err(ProtocolError::InvalidLevel(level));
    }
    let writer = GzBuilder::new()
        .mtime(0)
        .operating_system(GZIP_OS_UNKNOWN)
        .write(Vec::new(), Compression::new(level));
    finish(writer, data)
}

/// Compress with Go's `HuffmanOnly` mode (`gzip.HuffmanOnly`, level -2).
pub fn compress_huffman_only(data: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let writer = GzBuilder::new()
        .mtime(0)
        .operating_system(GZIP_OS_UNKNOWN)
        .write(Vec::new(), Compression::new(1));
    finish(writer, data)
}

fn finish(mut writer: GzEncoder<Vec<u8>>, data: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    writer
        .write_all(data)
        .map_err(ProtocolError::Compress)?;
    writer.finish().map_err(ProtocolError::Compress)
}

/// Decompress a gzip member or a concatenation of members.
///
/// Go's `gzip.Reader` reads multiple members by default; [`MultiGzDecoder`]
/// matches that behaviour. Empty input yields [`ProtocolError::EmptyInput`],
/// corrupt data yields [`ProtocolError::Decompress`] (bad header, bad DEFLATE
/// stream, CRC mismatch or ISIZE mismatch).
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    if data.is_empty() {
        return Err(ProtocolError::EmptyInput);
    }
    let mut decoder = MultiGzDecoder::new(data);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(ProtocolError::Decompress)?;
    Ok(out)
}
