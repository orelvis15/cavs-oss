//! Weak rolling hash (rsync-style Adler variant).
//!
//! Used only as a prefilter when scanning for reusable ranges: a weak match
//! is always confirmed with BLAKE3 before any byte is trusted. The identity
//! hash of CAVS stays BLAKE3-256 everywhere.
//!
//! Definition over a window `x_0..x_{l-1}` (all mod 2^16):
//!
//! ```text
//! a = Σ x_i
//! b = Σ (l - i) · x_i
//! weak32 = a | (b << 16)
//! ```
//!
//! Rolling one byte (drop `out`, append `in`) is O(1):
//! `a' = a - out + in`, `b' = b - l·out + a'`.

const MOD_MASK: u32 = 0xffff;

/// Weak hash of a full window in one pass.
pub fn weak32(window: &[u8]) -> u32 {
    let len = window.len() as u32;
    let mut a: u32 = 0;
    let mut b: u32 = 0;
    for (i, &x) in window.iter().enumerate() {
        a = a.wrapping_add(x as u32);
        b = b.wrapping_add((len - i as u32).wrapping_mul(x as u32));
    }
    (a & MOD_MASK) | ((b & MOD_MASK) << 16)
}

/// Incremental form of [`weak32`] over a fixed-size sliding window.
#[derive(Debug, Clone)]
pub struct RollingWeak {
    a: u32,
    b: u32,
    len: u32,
}

impl RollingWeak {
    pub fn new(window: &[u8]) -> Self {
        let len = window.len() as u32;
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        for (i, &x) in window.iter().enumerate() {
            a = a.wrapping_add(x as u32);
            b = b.wrapping_add((len - i as u32).wrapping_mul(x as u32));
        }
        Self { a, b, len }
    }

    /// Slide the window one byte: drop `out` from the front, append `inp`.
    #[inline]
    pub fn roll(&mut self, out: u8, inp: u8) {
        self.a = self.a.wrapping_sub(out as u32).wrapping_add(inp as u32);
        self.b = self
            .b
            .wrapping_sub(self.len.wrapping_mul(out as u32))
            .wrapping_add(self.a);
    }

    #[inline]
    pub fn digest(&self) -> u32 {
        (self.a & MOD_MASK) | ((self.b & MOD_MASK) << 16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let mut state = seed;
        for b in out.iter_mut() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (state >> 24) as u8;
        }
        out
    }

    #[test]
    fn known_vectors() {
        // Pinned values: if these change, on-disk signatures change too.
        assert_eq!(weak32(b""), 0);
        assert_eq!(weak32(b"a"), (97) | (97 << 16));
        // a = 97+98+99 = 0x126; b = 3·97 + 2·98 + 1·99 = 586 = 0x24a.
        assert_eq!(weak32(b"abc"), 0x024a_0126);
        // a = 64·255 = 0x3fc0; b = 255·(64+63+…+1) = 530400 ≡ 0x17e0 (mod 2^16).
        assert_eq!(weak32(&[0xff; 64]), 0x17e0_3fc0);
    }

    #[test]
    fn rolling_equals_recompute() {
        let data = pseudo_random(4096, 42);
        let window = 512;
        let mut rw = RollingWeak::new(&data[0..window]);
        assert_eq!(rw.digest(), weak32(&data[0..window]));
        for pos in 0..data.len() - window {
            rw.roll(data[pos], data[pos + window]);
            assert_eq!(
                rw.digest(),
                weak32(&data[pos + 1..pos + 1 + window]),
                "diverged at pos {pos}"
            );
        }
    }

    #[test]
    fn distinct_windows_usually_differ() {
        let data = pseudo_random(64 * 1024, 7);
        let a = weak32(&data[0..1024]);
        let b = weak32(&data[1024..2048]);
        assert_ne!(a, b);
    }
}
