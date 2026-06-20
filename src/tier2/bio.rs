//! The packet-header bit reader (ISO/IEC 15444-1 Annex B.10.1).
//!
//! Packet-header values — the tag-tree bits, inclusion bits, coding-pass and
//! length fields — are packed MSB-first into bytes with a stuffing rule: once a
//! byte equals `0xFF`, the most significant bit of the following byte is a
//! stuffed `0` and is skipped, so the header can never accidentally spell a
//! marker (`0xFF90`–`0xFFFF`). This reader is the decode side of that packing;
//! the Tier-2 packet parser drives it, and the [tag tree](super::tagtree) reads
//! one bit at a time through it.

/// An MSB-first bit reader over a packet header's bytes, honouring the Annex
/// B.10.1 `0xFF` bit-stuffing. Past the end of `data` it yields `1` bits (the
/// `0xFF` fill convention), which also bounds every read so a truncated header
/// can never index out of range or panic.
#[derive(Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    /// A 16-bit window: the previously consumed byte in bits 8..=15, the current
    /// byte in bits 0..=7 (matching OpenJPEG's `opj_bio` convention).
    buf: u32,
    /// Bits left to read from `buf`'s low byte before the next [`bytein`].
    ct: i32,
}

impl<'a> BitReader<'a> {
    /// A reader positioned at the first bit of `data`.
    pub fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            pos: 0,
            buf: 0,
            ct: 0,
        }
    }

    /// Pull the next byte into the window. If the byte just shifted into the
    /// high position is `0xFF`, the incoming byte contributes only seven bits
    /// (its stuffed MSB is dropped). Past the end the byte reads as `0xFF`.
    fn bytein(&mut self) {
        self.buf = (self.buf << 8) & 0xFFFF;
        self.ct = if self.buf == 0xFF00 { 7 } else { 8 };
        let byte = self.data.get(self.pos).copied().unwrap_or(0xFF);
        if self.pos < self.data.len() {
            self.pos += 1;
        }
        self.buf |= byte as u32;
    }

    /// Read one bit (MSB-first). Reading past the end of `data` yields `1`.
    pub fn read_bit(&mut self) -> u32 {
        if self.ct == 0 {
            self.bytein();
        }
        self.ct -= 1;
        (self.buf >> self.ct) & 1
    }

    /// Read `n` bits (`0..=32`) MSB-first into the low bits of the result.
    pub fn read(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for i in (0..n).rev() {
            v |= self.read_bit() << i;
        }
        v
    }

    /// Byte-align at the end of a packet header (Annex B.10.1, OpenJPEG
    /// `opj_bio_inalign`): drop any partially-read byte, and if the last whole
    /// byte was `0xFF` consume the following stuffed byte too (its bits are not
    /// header content). After this, [`bytes_consumed`](Self::bytes_consumed)
    /// reports where the packet body begins.
    pub fn align(&mut self) {
        if self.buf & 0xFF == 0xFF {
            self.bytein();
        }
        self.ct = 0;
    }

    /// Whole bytes consumed from `data` so far. Meaningful at a byte boundary —
    /// before the first read, or after [`align`](Self::align). A partially-read
    /// byte (`ct > 0`) counts as not yet consumed.
    pub fn bytes_consumed(&self) -> usize {
        if self.ct == 0 { self.pos } else { self.pos - 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::BitReader;

    /// Bits come out most significant first, byte by byte.
    #[test]
    fn reads_msb_first() {
        let mut b = BitReader::new(&[0b1011_0010, 0b0100_0000]);
        let bits: Vec<u32> = (0..8).map(|_| b.read_bit()).collect();
        assert_eq!(bits, [1, 0, 1, 1, 0, 0, 1, 0]);
        // The first two bits of the next byte continue the stream.
        assert_eq!(b.read(2), 0b01);
    }

    /// `read(n)` packs `n` MSB-first bits into the low end of the result.
    #[test]
    fn read_multiple_bits() {
        let mut b = BitReader::new(&[0xA5, 0x3C]);
        assert_eq!(b.read(4), 0xA); // 1010
        assert_eq!(b.read(4), 0x5); // 0101
        assert_eq!(b.read(8), 0x3C);
    }

    /// After a `0xFF` byte the next byte's most significant bit is stuffed and
    /// skipped (Annex B.10.1): the byte supplies only seven bits.
    #[test]
    fn ff_byte_stuffs_following_msb() {
        // 0xFF = eight 1s; then 0x80 = 1000_0000, whose stuffed MSB is dropped,
        // leaving seven 0 bits.
        let mut b = BitReader::new(&[0xFF, 0x80]);
        for _ in 0..8 {
            assert_eq!(b.read_bit(), 1, "first byte is all ones");
        }
        for _ in 0..7 {
            assert_eq!(b.read_bit(), 0, "stuffed byte yields seven zeros");
        }
    }

    /// Reading past the end yields `1` bits and never panics or runs `pos` away.
    #[test]
    fn past_end_is_bounded() {
        let mut b = BitReader::new(&[0x00]);
        // The single byte's eight 0 bits, then the 0xFF fill convention.
        for _ in 0..8 {
            assert_eq!(b.read_bit(), 0);
        }
        for _ in 0..64 {
            assert_eq!(b.read_bit(), 1);
        }
    }

    /// An empty input is immediately in the fill region; it must not panic.
    #[test]
    fn empty_input_is_bounded() {
        let mut b = BitReader::new(&[]);
        for _ in 0..16 {
            let _ = b.read_bit();
        }
    }

    /// Aligning after a partial byte discards the remaining bits and counts the
    /// byte as consumed; the next byte is where a packet body would start.
    #[test]
    fn align_rounds_up_partial_byte() {
        let mut b = BitReader::new(&[0b1011_0000, 0xAB]);
        assert_eq!(b.read(4), 0b1011);
        b.align();
        assert_eq!(b.bytes_consumed(), 1);
    }

    /// A header byte of `0xFF` stuffs the next byte: aligning must skip that
    /// whole stuffed byte (Annex B.10.1), so the body begins one byte later.
    #[test]
    fn align_skips_stuffed_byte_after_ff() {
        let mut b = BitReader::new(&[0xFF, 0x80, 0xAB]);
        for _ in 0..8 {
            assert_eq!(b.read_bit(), 1);
        }
        b.align();
        // 0xFF consumed, then the stuffed 0x80 skipped: body starts at index 2.
        assert_eq!(b.bytes_consumed(), 2);
    }

    /// Aligning when already on a byte boundary (and not after `0xFF`) consumes
    /// nothing further.
    #[test]
    fn align_on_boundary_is_noop() {
        let mut b = BitReader::new(&[0x3C, 0xAB]);
        assert_eq!(b.read(8), 0x3C);
        b.align();
        assert_eq!(b.bytes_consumed(), 1);
    }
}
