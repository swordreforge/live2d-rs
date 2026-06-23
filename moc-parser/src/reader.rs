use crate::error::{MocError, MocResult};

/// Big-endian binary reader for the MOC format.
///
/// Provides VLQ (variable-length quantity) integers, bit-level I/O,
/// and primitive-type reads.  This is the **only** layer that touches
/// raw bytes — callers (schema implementations) work exclusively
/// through this reader.
#[derive(Debug)]
pub struct BinaryReader<'a> {
    buf: &'a [u8],
    offset: usize,
    /// Stashed byte for bit-level reads.
    bit_byte: u8,
    /// Number of bits remaining in `bit_byte` (0 = byte-aligned).
    bits_remaining: u8,
}

impl<'a> BinaryReader<'a> {
    /// Create a new reader from raw MOC file bytes.
    ///
    /// # Errors
    /// Returns `InvalidMagic` if the first 4 bytes aren't `b"moc"` + version.
    pub fn new(buf: &'a [u8]) -> MocResult<Self> {
        if buf.len() < 4 {
            let mut magic = [0u8; 4];
            magic[..buf.len()].copy_from_slice(buf);
            return Err(MocError::UnexpectedEof {
                offset: 0,
                expected: 4,
                available: buf.len(),
            });
        }
        if buf[0] != b'm' || buf[1] != b'o' || buf[2] != b'c' {
            let mut magic = [0u8; 4];
            let copy_len = buf.len().min(4);
            magic[..copy_len].copy_from_slice(&buf[..copy_len]);
            return Err(MocError::InvalidMagic { actual: magic });
        }
        let version = buf[3];
        // Version 8..=11 are supported.
        // (Some MOC3-derived files incorrectly claim version < 8.)
        if !(8..=11).contains(&version) {
            return Err(MocError::UnsupportedVersion { version });
        }
        Ok(Self {
            buf,
            offset: 4, // skip magic + version
            bit_byte: 0,
            bits_remaining: 0,
        })
    }

    /// The MOC format version (typically 8–11).
    pub fn version(&self) -> u8 {
        self.buf[3]
    }

    /// Current byte offset into the buffer.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Number of bytes remaining.
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.offset)
    }

    // ── primitive reads ────────────────────────────────────────────

    /// Read one unsigned byte.
    pub fn read_u8(&mut self) -> MocResult<u8> {
        self.ensure(1)?;
        let v = self.buf[self.offset];
        self.offset += 1;
        Ok(v)
    }

    /// Read `count` bytes into a `&[u8]` slice.
    pub fn read_bytes(&mut self, count: usize) -> MocResult<&'a [u8]> {
        self.ensure(count)?;
        let slice = &self.buf[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    /// Read a big-endian `i16`.
    pub fn read_i16(&mut self) -> MocResult<i16> {
        self.ensure(2)?;
        let v = i16::from_be_bytes([self.buf[self.offset], self.buf[self.offset + 1]]);
        self.offset += 2;
        Ok(v)
    }

    /// Read a big-endian `u16`.
    pub fn read_u16(&mut self) -> MocResult<u16> {
        self.ensure(2)?;
        let v = u16::from_be_bytes([self.buf[self.offset], self.buf[self.offset + 1]]);
        self.offset += 2;
        Ok(v)
    }

    /// Read a big-endian `i32`.
    pub fn read_i32(&mut self) -> MocResult<i32> {
        self.ensure(4)?;
        let v = i32::from_be_bytes([
            self.buf[self.offset],
            self.buf[self.offset + 1],
            self.buf[self.offset + 2],
            self.buf[self.offset + 3],
        ]);
        self.offset += 4;
        Ok(v)
    }

    /// Read a big-endian `u32`.
    pub fn read_u32(&mut self) -> MocResult<u32> {
        self.ensure(4)?;
        let v = u32::from_be_bytes([
            self.buf[self.offset],
            self.buf[self.offset + 1],
            self.buf[self.offset + 2],
            self.buf[self.offset + 3],
        ]);
        self.offset += 4;
        Ok(v)
    }

    /// Read a big-endian `f32`.
    pub fn read_f32(&mut self) -> MocResult<f32> {
        self.read_u32().map(f32::from_bits)
    }

    /// Read a UTF-8 string: VLQ length prefix followed by that many bytes.
    pub fn read_string(&mut self) -> MocResult<&'a str> {
        let len = self.read_vlq()? as usize;
        let bytes = self.read_bytes(len)?;
        std::str::from_utf8(bytes).map_err(|e| MocError::InvalidLayout {
            context: "read_string",
            detail: format!("invalid UTF-8 at offset {}: {e}", self.offset - len),
        })
    }

    // ── VLQ (variable-length quantity) ─────────────────────────────

    /// Read a MOC variable-length integer (big-endian, 1–4 bytes).
    ///
    /// Encoding (Python `readNumber`):
    ///   byte 1  bit7=0 → 7-bit value
    ///   byte 1  bit7=1 + byte 2  bit7=0 → 14-bit value
    ///   byte 1  bit7=1 + byte 2  bit7=1 + byte 3  bit7=0 → 21-bit value
    ///   byte 1  bit7=1 + byte 2  bit7=1 + byte 3  bit7=1 + byte 4 → 28-bit value
    pub fn read_vlq(&mut self) -> MocResult<i32> {
        let b0 = self.read_u8()?;

        if b0 & 0x80 == 0 {
            // 1 byte: 7-bit value
            return Ok(b0 as i32);
        }

        let b1 = self.read_u8()?;
        if b1 & 0x80 == 0 {
            // 2 bytes: 14-bit value
            return Ok(((b0 as i32 & 0x7F) << 7) | b1 as i32);
        }

        let b2 = self.read_u8()?;
        if b2 & 0x80 == 0 {
            // 3 bytes: 21-bit value
            return Ok(((b0 as i32 & 0x7F) << 14) | ((b1 as i32 & 0x7F) << 7) | b2 as i32);
        }

        let b3 = self.read_u8()?;
        // 4 bytes: 28-bit value
        Ok(((b0 as i32 & 0x7F) << 21)
            | ((b1 as i32 & 0x7F) << 14)
            | ((b2 as i32 & 0x7F) << 7)
            | b3 as i32)
    }

    // ── Bit-level reads ────────────────────────────────────────────

    /// Ensure the next read is byte-aligned (discard partial byte).
    pub fn align_to_byte(&mut self) {
        self.bits_remaining = 0;
    }

    /// Read one bit.
    pub fn read_bit(&mut self) -> MocResult<bool> {
        if self.bits_remaining == 0 {
            self.bit_byte = self.read_u8()?;
            self.bits_remaining = 8;
        }
        let bit = (self.bit_byte & 0x80) != 0;
        self.bit_byte <<= 1;
        self.bits_remaining -= 1;
        Ok(bit)
    }

    // ── internal helpers ───────────────────────────────────────────

    fn ensure(&self, count: usize) -> MocResult<()> {
        let available = self.buf.len().saturating_sub(self.offset);
        if available < count {
            return Err(MocError::UnexpectedEof {
                offset: self.offset,
                expected: count,
                available,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_magic_too_short() {
        let err = BinaryReader::new(b"mo").unwrap_err();
        assert!(
            matches!(&err, MocError::UnexpectedEof { .. }),
            "expected UnexpectedEof, got {err:?}"
        );
    }

    #[test]
    fn test_invalid_magic_wrong() {
        let err = BinaryReader::new(b"abc\x08").unwrap_err();
        assert!(
            matches!(&err, MocError::InvalidMagic { .. }),
            "expected InvalidMagic, got {err:?}"
        );
    }

    #[test]
    fn test_unsupported_version() {
        let err = BinaryReader::new(b"moc\x07").unwrap_err();
        assert!(
            matches!(&err, MocError::UnsupportedVersion { version } if *version == 7),
            "expected UnsupportedVersion(7), got {err:?}"
        );
    }

    #[test]
    fn test_valid_version_8() {
        let mut r = BinaryReader::new(b"moc\x08\x00\x00\x00\x01").unwrap();
        assert_eq!(r.version(), 8);
        assert_eq!(r.read_i32().unwrap(), 1);
    }

    #[test]
    fn test_read_i32() {
        let mut r = BinaryReader::new(b"moc\x08\x00\x00\x00\xff\xff\xff\xff\xff\x80").unwrap();
        assert_eq!(r.read_i32().unwrap(), 255);
        assert_eq!(r.read_i32().unwrap(), -1);
        assert_eq!(r.read_u8().unwrap(), 0x80);
    }

    #[test]
    fn test_read_f32() {
        let mut r = BinaryReader::new(b"moc\x08\x40\x49\x0f\xdb").unwrap(); // π
        let pi = r.read_f32().unwrap();
        assert!((pi - std::f32::consts::PI).abs() < 0.001);
    }

    #[test]
    fn test_read_vlq_1byte() {
        let mut r = BinaryReader::new(b"moc\x08\x7f").unwrap();
        assert_eq!(r.read_vlq().unwrap(), 0x7f);
    }

    #[test]
    fn test_read_vlq_2byte() {
        let mut r = BinaryReader::new(b"moc\x08\x81\x01").unwrap();
        assert_eq!(r.read_vlq().unwrap(), 129); // (1 << 7) | 1 = 129
    }

    #[test]
    fn test_read_vlq_4byte() {
        let mut r = BinaryReader::new(b"moc\x08\xff\xff\xff\xff").unwrap();
        assert_eq!(r.read_vlq().unwrap(), 0x0FFF_FFFF);
    }

    #[test]
    fn test_read_bit() {
        let mut r = BinaryReader::new(b"moc\x08\xC0").unwrap(); // 0xC0 = 0b1100_0000
        assert!(r.read_bit().unwrap());  // 1
        assert!(r.read_bit().unwrap());  // 1
        assert!(!r.read_bit().unwrap()); // 0
        assert!(!r.read_bit().unwrap()); // 0
        assert!(!r.read_bit().unwrap()); // 0
        assert!(!r.read_bit().unwrap()); // 0
        assert!(!r.read_bit().unwrap()); // 0
        assert!(!r.read_bit().unwrap()); // 0
    }

    #[test]
    fn test_read_eof() {
        let mut r = BinaryReader::new(b"moc\x08").unwrap();
        let err = r.read_i32().unwrap_err();
        assert!(
            matches!(&err, MocError::UnexpectedEof { .. }),
            "expected UnexpectedEof, got {err:?}"
        );
    }

    #[test]
    fn test_read_string() {
        // VLQ(5) + b"hello"
        let mut data = Vec::from(&b"moc\x08"[..]);
        data.push(5); // len VLQ
        data.extend_from_slice(b"hello");
        let mut r = BinaryReader::new(&data).unwrap();
        assert_eq!(r.read_string().unwrap(), "hello");
    }
}
