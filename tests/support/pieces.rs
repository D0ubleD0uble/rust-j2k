//! The stream of pieces the `header_split` fuzz target feeds its input in,
//! and the changes of options between them: [`Pieces`]. The target and
//! `tests/jp2_box_error_final.rs` include this file with `#[path]`, so a
//! test pins what a committed seed's stream does in the target itself.
//!
//! This file has no dependency, so the fuzz target includes it as it does
//! `held.rs`.

#![allow(dead_code)]

/// splitmix64: the same stream on every platform, from a seed in the input.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// A change of options after a feed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// The region, from two bytes (the target's `region`).
    Region { code: u8, arg: u8 },
    /// The resolution reduction, 0 to 2.
    Reduction(u8),
    /// The layer cap, 1 to 3.
    Layers(u16),
}

/// One feed: the input is fed up to `to`, then `change` is made, if any,
/// and `again` says whether `header()` is called a second time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Feed {
    pub to: usize,
    pub change: Option<Change>,
    pub again: bool,
}

/// The feeds of an input of `len` bytes after the control bytes `control`
/// (bytes 6 to 11 seed the stream): pieces of 1 to twice `len / 24`
/// bytes, so about 24 feeds, each followed by a change one time in eight.
pub struct Pieces {
    rng: Rng,
    most: usize,
    len: usize,
    fed: usize,
}

impl Pieces {
    pub fn new(control: &[u8], len: usize) -> Self {
        let c = control;
        Self {
            rng: Rng(u64::from_le_bytes([
                c[6], c[7], c[8], c[9], c[10], c[11], 0, 0,
            ])),
            most: (len / 24).max(1),
            len,
            fed: 0,
        }
    }
}

impl Iterator for Pieces {
    type Item = Feed;

    fn next(&mut self) -> Option<Feed> {
        if self.fed >= self.len {
            return None;
        }
        let piece = 1 + (self.rng.next() as usize) % (2 * self.most);
        self.fed = (self.fed + piece).min(self.len);
        let r = self.rng.next();
        let arg = (r >> 8) as u8;
        let change = r.is_multiple_of(8).then_some(match (r >> 16) % 3 {
            0 => Change::Region {
                code: (r >> 24) as u8,
                arg,
            },
            1 => Change::Reduction(arg % 3),
            _ => Change::Layers(1 + u16::from(arg % 3)),
        });
        Some(Feed {
            to: self.fed,
            change,
            again: r.is_multiple_of(5),
        })
    }
}
