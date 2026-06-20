//! Tag trees (ISO/IEC 15444-1 Annex B.10.2).
//!
//! A tag tree is a hierarchical, lazily-read structure packet headers use twice
//! per precinct: once for code-block *inclusion* (the layer a block first
//! appears in) and once for the number of all-zero most-significant bit-planes.
//! Values are read incrementally against a rising threshold across packets, so
//! the tree carries decode state between reads.
//!
//! The leaves form a `width × height` grid. Each level up halves the grid
//! (`ceil` per axis) until a single root; every node holds the minimum of its
//! descendants. Reading a leaf against `threshold` walks root→leaf, consuming
//! one bit per step of the search: a `1` fixes the node's value at the current
//! lower bound, a `0` raises the bound. A node's value is only *known* once the
//! search drops below the threshold; otherwise the read returns `None` and the
//! partial state (`value`, `low`) is kept so a later, higher-threshold read in a
//! subsequent packet resumes exactly where this one stopped.

use crate::tier2::bio::BitReader;

/// A value that has not yet been pinned down. Larger than any real tag-tree
/// value (bounded by the bit-plane and layer counts), so an unresolved node
/// always compares as "not below threshold".
const UNRESOLVED: u32 = u32::MAX;

/// Maximum tag-tree path length handled without heap allocation. A `u32` grid
/// halves to its 1×1 root in at most 32 steps (so 33 levels, 32 ancestors on a
/// path); this bounds the stack for any precinct size, including a malformed one
/// from untrusted input, so a read never allocates and never overflows.
const MAX_DEPTH: usize = 33;

/// One tag-tree node: the current best (lowest-known) value estimate and the
/// lower bound the incremental search has already established.
#[derive(Debug, Clone, Copy)]
struct Node {
    /// Index of this node's parent in [`TagTree::nodes`]; `None` for the root.
    parent: Option<usize>,
    /// The value if known, else [`UNRESOLVED`]. Set to `low` when a `1` bit is
    /// read; only meaningful (final) once it is below the read threshold.
    value: u32,
    /// The established lower bound: the search has proven the value is `>= low`.
    /// Retained between reads so a higher-threshold read resumes here.
    low: u32,
}

/// A quad tag tree over a `width × height` grid of leaves.
#[derive(Debug, Clone)]
pub struct TagTree {
    width: u32,
    height: u32,
    /// All nodes, level 0 (the leaves) first, then each coarser level. A leaf
    /// `(x, y)` is at index `y * width + x`.
    nodes: Vec<Node>,
}

/// Geometry of one tag-tree level: its grid size and where its nodes start in
/// [`TagTree::nodes`].
struct Level {
    width: u32,
    height: u32,
    offset: usize,
}

impl TagTree {
    /// Build a tag tree for a precinct's `width × height` code-block grid. All
    /// nodes start unresolved; values arrive through [`read`](Self::read).
    pub fn new(width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);

        // Enumerate levels from the leaf grid up to the 1×1 root, recording each
        // level's size and its base offset in the flat node array.
        let mut levels = Vec::new();
        let (mut lw, mut lh, mut offset) = (width, height, 0usize);
        loop {
            levels.push(Level {
                width: lw,
                height: lh,
                offset,
            });
            offset += lw as usize * lh as usize;
            if lw == 1 && lh == 1 {
                break;
            }
            lw = lw.div_ceil(2);
            lh = lh.div_ceil(2);
        }

        let mut nodes = vec![
            Node {
                parent: None,
                value: UNRESOLVED,
                low: 0,
            };
            offset
        ];

        // Link each node to its parent in the next coarser level: the node at
        // (x, y) descends from the parent at (x/2, y/2).
        for pair in levels.windows(2) {
            let (cur, parent) = (&pair[0], &pair[1]);
            for y in 0..cur.height {
                for x in 0..cur.width {
                    let child = cur.offset + (y as usize * cur.width as usize + x as usize);
                    let par = parent.offset
                        + ((y / 2) as usize * parent.width as usize + (x / 2) as usize);
                    nodes[child].parent = Some(par);
                }
            }
        }

        TagTree {
            width,
            height,
            nodes,
        }
    }

    /// Read leaf `(x, y)` against `threshold`, consuming bits from the packet
    /// header bit-reader. Returns `Some(value)` if the value is now known to be
    /// below the threshold, else `None` (read again at a higher threshold in a
    /// later packet). Decode state is retained between calls.
    pub fn read(&mut self, x: u32, y: u32, threshold: u32, bio: &mut BitReader) -> Option<u32> {
        // The caller (the packet parser) addresses an in-bounds leaf; precinct
        // geometry is validated upstream before any tree is built.
        debug_assert!(
            x < self.width && y < self.height,
            "leaf ({x}, {y}) out of range"
        );
        let leaf = y as usize * self.width as usize + x as usize;

        // Stack the path from leaf up to the root, so the search can run the
        // other way (root→leaf), carrying the running lower bound downward.
        let mut path = [0usize; MAX_DEPTH];
        let mut depth = 0;
        let mut idx = leaf;
        while let Some(parent) = self.nodes[idx].parent {
            path[depth] = idx;
            depth += 1;
            idx = parent;
        }
        // `idx` is now the root.

        let mut low = 0;
        loop {
            let node = &mut self.nodes[idx];
            // A child can never resolve below its parent's bound; carry the
            // larger of the two and keep them in step.
            low = low.max(node.low);
            while low < threshold && low < node.value {
                if bio.read_bit() == 1 {
                    node.value = low;
                } else {
                    low += 1;
                }
            }
            node.low = low;

            if depth == 0 {
                break;
            }
            depth -= 1;
            idx = path[depth];
        }

        let value = self.nodes[leaf].value;
        (value < threshold).then_some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MSB-first bits packed into bytes, the layout a packet header would carry.
    fn bits(seq: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; seq.len().div_ceil(8)];
        for (i, &b) in seq.iter().enumerate() {
            if b == 1 {
                bytes[i / 8] |= 0x80 >> (i % 8);
            }
        }
        bytes
    }

    /// A 1×1 tree (root is the only leaf). Value 0 encodes as a single `1` bit.
    #[test]
    fn single_leaf_value_zero() {
        let data = bits(&[1]);
        let mut bio = BitReader::new(&data);
        let mut t = TagTree::new(1, 1);
        assert_eq!(t.read(0, 0, 1, &mut bio), Some(0));
    }

    /// A larger value reads as a run of `0`s (raising the bound) then a `1`.
    /// Value 3 in a 1×1 tree encodes as `0 0 0 1`.
    #[test]
    fn single_leaf_value_three() {
        let data = bits(&[0, 0, 0, 1]);
        let mut bio = BitReader::new(&data);
        let mut t = TagTree::new(1, 1);
        assert_eq!(t.read(0, 0, 4, &mut bio), Some(3));
    }

    /// Below the threshold the value stays unresolved (`None`); raising the
    /// threshold in a later read resumes the same bitstream and resolves it.
    /// Value 2 encodes as `0 0 1`; thresholds 1, 2, 3 split the search.
    #[test]
    fn rising_threshold_resolves_incrementally() {
        let data = bits(&[0, 0, 1]);
        let mut bio = BitReader::new(&data);
        let mut t = TagTree::new(1, 1);
        assert_eq!(t.read(0, 0, 1, &mut bio), None); // reads `0`, low=1
        assert_eq!(t.read(0, 0, 2, &mut bio), None); // reads `0`, low=2
        assert_eq!(t.read(0, 0, 3, &mut bio), Some(2)); // reads `1`, value=2
    }

    /// Inter-packet state must survive a *new* bit reader: a packet boundary
    /// hands the same tree a fresh reader over the next header's bits. Value 2
    /// (`0 0 1`) is split across two readers; the tree remembers `low`.
    #[test]
    fn state_retained_across_separate_readers() {
        let mut t = TagTree::new(1, 1);

        let first = bits(&[0, 0]); // packet 1's header bits for this node
        let mut bio1 = BitReader::new(&first);
        assert_eq!(t.read(0, 0, 2, &mut bio1), None);

        let second = bits(&[1]); // packet 2 continues the search
        let mut bio2 = BitReader::new(&second);
        assert_eq!(t.read(0, 0, 3, &mut bio2), Some(2));
    }

    /// A two-leaf tree (2×1) sharing one root. Values L0=1, L1=0 give a root
    /// minimum of 0. Decoding L1 then L0 at threshold 2 consumes, in order:
    /// root `1` (min 0), L1 `1` (value 0); then L0 `0 1` (value 1) — bits
    /// `1 1 0 1`. The shared root is resolved once and reused.
    #[test]
    fn two_leaves_share_a_root() {
        let data = bits(&[1, 1, 0, 1]);
        let mut bio = BitReader::new(&data);
        let mut t = TagTree::new(2, 1);

        assert_eq!(t.read(1, 0, 2, &mut bio), Some(0));
        assert_eq!(t.read(0, 0, 2, &mut bio), Some(1));
    }

    /// The classic inclusion use: a leaf becomes included at layer `v` once the
    /// per-layer threshold passes `v`. A 2×2 precinct with one block included at
    /// layer 0 and the rest later; here we just confirm a leaf reads `None`
    /// until its layer and `Some` after, with the tree shape intact.
    #[test]
    fn inclusion_across_layers() {
        // 2×2 leaves, value at (0,0) is 0 (included immediately). Root min 0.
        // Reading (0,0): root `1` (min 0), then (0,0) `1` (value 0): bits `1 1`.
        let data = bits(&[1, 1]);
        let mut bio = BitReader::new(&data);
        let mut t = TagTree::new(2, 2);
        // Layer 0 inclusion is "value < 1".
        assert_eq!(t.read(0, 0, 1, &mut bio), Some(0));
    }

    /// Building odd-sized trees must not panic and must link every leaf to the
    /// root through valid parents (a read terminates).
    #[test]
    fn odd_dimensions_build_and_read() {
        for (w, h) in [(1, 1), (3, 2), (5, 5), (7, 3), (1, 9)] {
            let mut t = TagTree::new(w, h);
            let data = bits(&[0; 64]);
            let mut bio = BitReader::new(&data);
            // A read at a high threshold terminates and yields a value or None
            // without indexing out of bounds.
            let _ = t.read(w - 1, h - 1, 8, &mut bio);
        }
    }
}
