//! Spans of tag-tree leaves a packet header passes over without reading a
//! bit, and the cache that carries them from one row of a pass to the next
//! (see [`TagTree::unread_span`]).

use crate::tier2::tagtree::TagTree;

/// A rectangle of tag-tree leaves under one node: columns `x0..x1`, rows
/// `y0..y1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub x0: u32,
    pub x1: u32,
    pub y0: u32,
    pub y1: u32,
}

impl Span {
    /// The next entry, in raster order over a `cols`-wide grid, after the
    /// leaves of this span from row `y` on: the rest of row `y` within the
    /// span, or, when the span covers the whole row, every row left in it.
    pub fn next_entry(&self, y: u32, cols: u32) -> usize {
        if self.x0 == 0 && self.x1 == cols {
            self.y1 as usize * cols as usize
        } else {
            y as usize * cols as usize + self.x1 as usize
        }
    }

    /// The leaves under the level-`level` node above leaf `(x, y)`, in a
    /// `width × height` grid. Each level halves the grid, so that node is
    /// `(x >> level, y >> level)` on its level.
    pub(crate) fn of(x: u32, y: u32, level: u32, width: u32, height: u32) -> Span {
        let side = 1u64 << level;
        let x0 = (u64::from(x) >> level) << level;
        let y0 = (u64::from(y) >> level) << level;
        Span {
            x0: x0 as u32,
            x1: (x0 + side).min(u64::from(width)) as u32,
            y0: y0 as u32,
            y1: (y0 + side).min(u64::from(height)) as u32,
        }
    }
}

/// The unread spans of one pass over a tag tree's leaves in raster order at
/// one threshold, so that a span met on one row is stepped over on the rows
/// below it without walking the tree again.
///
/// A span stays unread for the rest of the pass: its node can only change
/// through a read of one of its leaves, and the pass reads none of them.
#[derive(Debug, Default)]
pub struct UnreadSpans {
    /// Spans met on the rows above that reach this row, mostly by column.
    above: Vec<Span>,
    /// The first entry of `above` that may still hold a leaf of this row.
    next: usize,
    /// Spans used on this row that reach the next.
    below: Vec<Span>,
    /// The row the pass is on.
    row: u32,
}

impl UnreadSpans {
    /// The span holding leaf `(x, y)` that a read at `threshold` passes over
    /// without reading a bit, or `None` if the leaf must be read (see
    /// [`TagTree::unread_span`]). The leaves must come in raster order, none
    /// read inside a span this returned.
    pub fn find(&mut self, tree: &TagTree, x: u32, y: u32, threshold: u32) -> Option<Span> {
        if y != self.row {
            std::mem::swap(&mut self.above, &mut self.below);
            self.below.clear();
            self.above.retain(|span| span.y1 > y);
            self.next = 0;
            self.row = y;
        }
        while self.above.get(self.next).is_some_and(|span| span.x1 <= x) {
            self.next += 1;
        }
        let span = match self.above.get(self.next) {
            Some(&span) if span.x0 <= x => span,
            _ => tree.unread_span(x, y, threshold)?,
        };
        if span.y1 > y + 1 {
            self.below.push(span);
        }
        Some(span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A span as wide as the grid is stepped over in one go, every row of it;
    /// a narrower one only to its end on the current row.
    #[test]
    fn a_full_width_span_steps_over_all_its_rows() {
        let full = Span::of(0, 0, 3, 8, 6);
        assert_eq!(
            full,
            Span {
                x0: 0,
                x1: 8,
                y0: 0,
                y1: 6
            }
        );
        assert_eq!(full.next_entry(0, 8), 48);
        assert_eq!(full.next_entry(4, 8), 48);
        // Clipped to a grid narrower than the node: still the full width.
        assert_eq!(Span::of(2, 1, 2, 3, 9).next_entry(1, 3), 12);

        let part = Span::of(5, 2, 2, 8, 8);
        assert_eq!(
            part,
            Span {
                x0: 4,
                x1: 8,
                y0: 0,
                y1: 4
            }
        );
        assert_eq!(part.next_entry(2, 8), 2 * 8 + 8);
        let left = Span::of(1, 3, 1, 8, 8);
        assert_eq!(
            left,
            Span {
                x0: 0,
                x1: 2,
                y0: 2,
                y1: 4
            }
        );
        assert_eq!(left.next_entry(3, 8), 3 * 8 + 2);
    }
}
