//! Unsigned LEB128 varints for the binary manifest.
//!
//! Encoding: 7 payload bits per byte, LSB first; the high bit marks
//! continuation. Decoding is strict: truncated input, more than
//! [`MAX_VARINT_BYTES`] bytes, bits beyond u64 and overlong encodings
//! (a redundant trailing `0x00` continuation byte) are all rejected, so
//! every value has exactly one valid wire form.

use crate::ManifestError;

/// A u64 never needs more than 10 LEB128 bytes.
pub const MAX_VARINT_BYTES: usize = 10;

pub fn write_varuint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Decode one varuint from the front of `input`, advancing it.
pub fn read_varuint(input: &mut &[u8]) -> Result<u64, ManifestError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for i in 0..MAX_VARINT_BYTES {
        let Some(&byte) = input.get(i) else {
            return Err(ManifestError::Truncated("varint"));
        };
        // Overlong: a continuation led here but this byte adds nothing.
        if byte == 0 && shift != 0 {
            return Err(ManifestError::VarintOverlong);
        }
        // The 10th byte may only carry the single remaining bit of a u64.
        if i == MAX_VARINT_BYTES - 1 && byte > 1 {
            return Err(ManifestError::VarintOverflow);
        }
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            *input = &input[i + 1..];
            return Ok(value);
        }
        shift += 7;
    }
    Err(ManifestError::VarintOverlong)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(value: u64) -> u64 {
        let mut buf = Vec::new();
        write_varuint(value, &mut buf);
        let mut slice = buf.as_slice();
        let decoded = read_varuint(&mut slice).unwrap();
        assert!(slice.is_empty(), "decoder must consume exactly one varint");
        decoded
    }

    #[test]
    fn varuint_round_trip_boundaries() {
        let cases = [
            0,
            1,
            127,
            128,
            255,
            256,
            16384,
            u32::MAX as u64,
            u64::MAX - 1,
            u64::MAX,
        ];
        for value in cases {
            assert_eq!(roundtrip(value), value);
        }
    }

    #[test]
    fn single_byte_values_use_one_byte() {
        let mut buf = Vec::new();
        write_varuint(127, &mut buf);
        assert_eq!(buf, [0x7f]);
    }

    #[test]
    fn rejects_truncated() {
        // Continuation bit set but no next byte.
        let mut slice: &[u8] = &[0x80];
        assert!(matches!(
            read_varuint(&mut slice),
            Err(ManifestError::Truncated(_))
        ));
        let mut empty: &[u8] = &[];
        assert!(read_varuint(&mut empty).is_err());
    }

    #[test]
    fn rejects_overlong() {
        // 0 encoded in two bytes: 0x80 0x00.
        let mut slice: &[u8] = &[0x80, 0x00];
        assert!(matches!(
            read_varuint(&mut slice),
            Err(ManifestError::VarintOverlong)
        ));
        // Eleven continuation bytes.
        let mut long: &[u8] = &[0xff; 11];
        assert!(read_varuint(&mut long).is_err());
    }

    #[test]
    fn rejects_u64_overflow() {
        // 10th byte with more than the one bit a u64 has left.
        let mut slice: &[u8] = &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];
        assert!(matches!(
            read_varuint(&mut slice),
            Err(ManifestError::VarintOverflow)
        ));
        // ...while the max u64 itself decodes fine.
        let mut ok: &[u8] = &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
        assert_eq!(read_varuint(&mut ok).unwrap(), u64::MAX);
    }
}
