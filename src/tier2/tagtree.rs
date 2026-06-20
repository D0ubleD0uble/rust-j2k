//! Tag trees (ISO/IEC 15444-1 Annex B.10.2).
//!
//! A tag tree is a hierarchical, lazily-read structure packet headers use twice
//! per precinct: once for code-block *inclusion* (the layer a block first
//! appears in) and once for the number of all-zero most-significant bit-planes.
//! Values are read incrementally against a rising threshold across packets, so
//! the tree carries decode state between reads.

/// A quad tag tree over a `width * height` grid of leaves.
#[derive(Debug, Clone)]
pub struct TagTree {
    width: u32,
    height: u32,
    // Per-node current value and known/partial state, level by level.
}

impl TagTree {
    /// Build a tag tree for a precinct's code-block grid.
    pub fn new(width: u32, height: u32) -> Self {
        TagTree { width, height }
    }

    /// Read leaf `(x, y)` against `threshold`, consuming bits from the packet
    /// header bit-reader. Returns `Some(value)` if the value is now known to be
    /// below the threshold, else `None` (read again at a higher threshold in a
    /// later packet).
    pub fn read(&mut self, x: u32, y: u32, threshold: u32 /*, reader */) -> Option<u32> {
        todo!("incremental tag-tree read with threshold and inter-packet state")
    }
}
