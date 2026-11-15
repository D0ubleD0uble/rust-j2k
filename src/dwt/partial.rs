//! The inverse DWT over part of a tile-component, for a decode window.
//!
//! A window needs only some samples of each resolution: the ones inside it at
//! the finest kept resolution, and at each coarser one the samples the
//! synthesis of the finer one reads. [`Footprint`] works those out from the
//! window, one axis at a time, finest resolution first. Tier-1 skips the
//! code-blocks that hold none of them ([`Footprint::prune`]), and
//! [`inverse_window`] runs the synthesis over them alone.
//!
//! The lifting is local: after a filter's `m` lifting steps a sample depends
//! only on the `m` samples on either side of it (`m` is 2 for 5/3 and 4 for
//! 9/7: the most extension ISO/IEC 15444-1 Tables F.2 and F.3 ask for at
//! either end, and the `filter_width` of OpenJPEG's
//! `opj_dwt_decode_partial_tile`).
//! So to get the samples `a..b` of a level, the synthesis runs over
//! `a - m..b + m`, cut to the level, with the same kernels the whole-tile
//! synthesis uses. Where the run is cut short of the level's edge, the
//! kernels' mirror extension reads the wrong neighbor, but each lifting step
//! carries that error only one sample further in, so after `m` steps it stays
//! in the `m` samples outside `a..b`. Every sample in `a..b` then goes through
//! the same arithmetic on the same values as in the whole-tile synthesis, so
//! the window is the crop of the whole decode exactly, on 9/7 as on 5/3.
//!
//! OpenJPEG maps the window to each band directly and grows it there by
//! `filter_width` band samples (`opj_dwt_decode_partial_tile`), or by 2 and 3
//! to pick code-blocks (`opj_tcd_is_subband_area_of_interest`). This module
//! grows it by `m` samples of each resolution, level by level, so it follows
//! what the synthesis reads and the code-blocks are picked from the same
//! samples. The code-blocks decoded can differ from OpenJPEG's; the samples
//! are still those of the whole decode.
//! `tests/decode_window_random.rs` checks random windows against the whole
//! decode.

use std::ops::Range;

use super::{Irreversible97, LANES, Reversible53, Synthesis, interleave};
use crate::codestream::markers::{Rect, Transform};
use crate::pipeline::{Band, Bands, DetailBands, Samples, SubbandCoeffs};
use crate::tier2::{BandKind, CodeBlock, ComponentCoded};
use crate::{Error, Result};

/// The samples a 5/3 synthesis sample depends on, on either side: its two
/// lifting steps.
const MARGIN_53: usize = 2;

/// The samples a 9/7 synthesis sample depends on, on either side: its four
/// lifting steps.
const MARGIN_97: usize = 4;

/// The samples a window needs of every resolution and subband of one
/// tile-component, and the samples the synthesis runs over to get them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Footprint {
    x: Axis,
    y: Axis,
}

/// One axis of a [`Footprint`]. Resolution `k` runs from 0, the coarsest
/// (the LL band), to the finest the decode keeps; every range counts samples
/// from the resolution's or band's first one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Axis {
    /// Each resolution's first sample on the tile-component grid.
    origin: Vec<u32>,
    /// Each resolution's size.
    size: Vec<usize>,
    /// The samples needed of each resolution: of the finest, those in the
    /// window; of each coarser one, the low-pass samples the next finer
    /// one's synthesis reads.
    need: Vec<Range<usize>>,
    /// For each resolution `k > 0`, at index `k - 1`: the samples of it the
    /// synthesis runs over.
    span: Vec<Range<usize>>,
    /// For each resolution `k > 0`, at index `k - 1`: the samples needed of
    /// its high-pass band.
    high: Vec<Range<usize>>,
}

/// The low-pass band samples among the samples `span` of a resolution whose
/// first sample's parity is `cas`. Resolution sample `i` is low-pass when
/// `i + cas` is even, and is sample `i / 2` of its band.
fn low(span: &Range<usize>, cas: usize) -> Range<usize> {
    (span.start + 1 - cas) / 2..(span.end + 1 - cas) / 2
}

/// The high-pass band samples among the samples `span`, as [`low`].
fn high(span: &Range<usize>, cas: usize) -> Range<usize> {
    (span.start + cas) / 2..(span.end + cas) / 2
}

impl Axis {
    /// The axis of resolutions at `origin` of `size` samples, coarsest first,
    /// for a window `target` on the finest one's grid, with a filter that
    /// reaches `margin` samples.
    fn new(origin: Vec<u32>, size: Vec<usize>, target: Range<u32>, margin: usize) -> Self {
        let finest = origin.len() - 1;
        let start = target.start.saturating_sub(origin[finest]) as usize;
        let end = (target.end.saturating_sub(origin[finest]) as usize).min(size[finest]);
        let mut need = vec![0..0; finest + 1];
        let mut span = vec![0..0; finest];
        let mut high_need = vec![0..0; finest];
        if start < end {
            need[finest] = start..end;
            for k in (1..=finest).rev() {
                let cas = (origin[k] & 1) as usize;
                let wanted = &need[k];
                // A level that needs no sample, below one whose run held no
                // low-pass sample, runs over none.
                let run = if wanted.is_empty() {
                    0..0
                } else {
                    wanted.start.saturating_sub(margin)..(wanted.end + margin).min(size[k])
                };
                let low_size = size[k - 1];
                let high_size = size[k] - low_size;
                let l = low(&run, cas);
                let h = high(&run, cas);
                need[k - 1] = l.start.min(low_size)..l.end.min(low_size);
                high_need[k - 1] = h.start.min(high_size)..h.end.min(high_size);
                span[k - 1] = run;
            }
        }
        Axis {
            origin,
            size,
            need,
            span,
            high: high_need,
        }
    }

    /// Whether `start..start + len` meets `range`.
    fn meets(range: &Range<usize>, start: usize, len: usize) -> bool {
        start < range.end && start + len > range.start
    }

    /// The finest resolution's index.
    fn finest(&self) -> usize {
        self.need.len() - 1
    }

    /// The window on the finest resolution, on the tile-component grid.
    fn target(&self) -> Range<u32> {
        let k = self.finest();
        let need = &self.need[k];
        self.origin[k] + need.start as u32..self.origin[k] + need.end as u32
    }
}

impl Footprint {
    /// The footprint of `target` on one tile-component whose coded data is
    /// `coded` and whose rectangle at `reduction` is `rect`, under
    /// `transform`. `None` when `target` covers the whole tile-component,
    /// which the whole-tile synthesis then decodes. `target` may reach past
    /// the tile-component, or miss it, which needs no sample.
    pub(crate) fn of(
        coded: &ComponentCoded<'_>,
        reduction: u8,
        rect: Rect,
        target: Rect,
        transform: Transform,
    ) -> Result<Option<Self>> {
        let clip = target.intersect(&rect);
        if clip == rect {
            return Ok(None);
        }
        let kept = coded
            .resolutions
            .len()
            .saturating_sub(usize::from(reduction))
            .max(1);
        let (mut xo, mut xs, mut yo, mut ys) = (vec![], vec![], vec![], vec![]);
        for res in 0..kept.min(coded.resolutions.len()) {
            let band = |kind| {
                coded.band(res, kind).ok_or_else(|| {
                    Error::Inconsistent(format!("resolution is missing its {kind:?} subband"))
                })
            };
            if res == 0 {
                let ll = band(BandKind::Ll)?;
                xo.push(ll.origin.0);
                yo.push(ll.origin.1);
                xs.push(ll.width);
                ys.push(ll.height);
            } else {
                let (hl, lh) = (band(BandKind::Hl)?, band(BandKind::Lh)?);
                // ISO Eq. B-15: a level's origin is its low-pass band's plus
                // its high-pass band's (see `merge_level`).
                xo.push(xo[res - 1] + hl.origin.0);
                yo.push(yo[res - 1] + lh.origin.1);
                xs.push(xs[res - 1] + hl.width);
                ys.push(ys[res - 1] + lh.height);
            }
        }
        let Some(finest) = xo.len().checked_sub(1) else {
            return Err(Error::Inconsistent(
                "tier-2 produced no resolutions to decode".into(),
            ));
        };
        if (xo[finest], yo[finest], xs[finest], ys[finest])
            != (
                rect.x0,
                rect.y0,
                rect.width() as usize,
                rect.height() as usize,
            )
        {
            return Err(Error::Inconsistent(format!(
                "a tile-component's subbands do not make up its rectangle {rect:?}"
            )));
        }
        let margin = match transform {
            Transform::Reversible53 => MARGIN_53,
            Transform::Irreversible97 => MARGIN_97,
        };
        let mut x = Axis::new(xo, xs, clip.x0..clip.x1, margin);
        let mut y = Axis::new(yo, ys, clip.y0..clip.y1, margin);
        // A window empty on one axis needs nothing on either.
        if x.need[finest].is_empty() || y.need[finest].is_empty() {
            x = Axis::new(x.origin, x.size, 0..0, margin);
            y = Axis::new(y.origin, y.size, 0..0, margin);
        }
        Ok(Some(Footprint { x, y }))
    }

    /// The window on the tile-component grid: the rectangle
    /// [`inverse_window`] reconstructs. It can be empty.
    pub(crate) fn rect(&self) -> Rect {
        let (x, y) = (self.x.target(), self.y.target());
        Rect {
            x0: x.start,
            y0: y.start,
            x1: x.end,
            y1: y.end,
        }
    }

    /// The samples needed of the `kind` band of resolution `res`, as column
    /// and row ranges from the band's first sample.
    fn band(&self, res: usize, kind: BandKind) -> (Range<usize>, Range<usize>) {
        let (x, y) = (&self.x, &self.y);
        match (res, kind) {
            (0, _) => (x.need[0].clone(), y.need[0].clone()),
            (_, BandKind::Hl) => (x.high[res - 1].clone(), y.need[res - 1].clone()),
            (_, BandKind::Lh) => (x.need[res - 1].clone(), y.high[res - 1].clone()),
            (_, _) => (x.high[res - 1].clone(), y.high[res - 1].clone()),
        }
    }

    /// Drop the code-blocks of `coded` that hold no sample the window needs,
    /// so Tier-1 neither decodes nor counts them: a block with no coding
    /// pass is left at zero. Those zeros are never read.
    pub(crate) fn prune(&self, coded: &mut ComponentCoded<'_>) {
        for (res, resolution) in coded
            .resolutions
            .iter_mut()
            .take(self.x.need.len())
            .enumerate()
        {
            for subband in &mut resolution.subbands {
                let kind = subband.kind;
                for block in &mut subband.blocks {
                    if !self.needs(res, kind, block) {
                        block.num_passes = 0;
                    }
                }
            }
        }
    }

    /// Whether `block`, of the `kind` band of resolution `res`, holds a
    /// sample the window needs. A block of a resolution the decode drops
    /// holds none.
    pub(crate) fn needs(&self, res: usize, kind: BandKind, block: &CodeBlock<'_>) -> bool {
        if res >= self.x.need.len() {
            return false;
        }
        let (columns, rows) = self.band(res, kind);
        Axis::meets(&columns, block.x, block.width) && Axis::meets(&rows, block.y, block.height)
    }
}

/// [`super::inverse`] over the samples `footprint` needs: the samples of
/// [`Footprint::rect`], row-major. `coeffs` must hold the subbands the
/// footprint was made from.
pub(crate) fn inverse_window(coeffs: SubbandCoeffs, footprint: &Footprint) -> Result<Samples> {
    Ok(match coeffs {
        SubbandCoeffs::Reversible(bands) => {
            Samples::Reversible(reconstruct::<Reversible53>(bands, footprint)?)
        }
        SubbandCoeffs::Irreversible(bands) => {
            Samples::Irreversible(reconstruct::<Irreversible97>(bands, footprint)?)
        }
    })
}

/// Part of one resolution: the samples `columns` by `rows` of it, row-major.
struct Region<T> {
    columns: Range<usize>,
    rows: Range<usize>,
    data: Vec<T>,
}

impl<T: Copy> Region<T> {
    /// The samples `columns` of row `row`, which the region holds.
    fn row(&self, row: usize, columns: &Range<usize>) -> &[T] {
        let width = self.columns.len();
        let start = (row - self.rows.start) * width + columns.start - self.columns.start;
        &self.data[start..start + columns.len()]
    }
}

/// The samples `columns` of row `row` of `band`.
fn band_row<'a, T>(band: &'a Band<T>, row: usize, columns: &Range<usize>) -> &'a [T] {
    let start = row * band.width + columns.start;
    &band.data[start..start + columns.len()]
}

/// Check that `bands` has the shape `footprint` was made from, so no range
/// of it reaches past a band.
fn check_shape<T>(bands: &Bands<T>, footprint: &Footprint) -> Result<()> {
    let (x, y) = (&footprint.x, &footprint.y);
    let inconsistent = || {
        Error::Inconsistent(
            "a tile-component's subbands changed shape between Tier-1 and the wavelet".into(),
        )
    };
    if bands.levels.len() != x.finest() {
        return Err(inconsistent());
    }
    let mut fits = (bands.ll.width, bands.ll.height) == (x.size[0], y.size[0])
        && bands.ll.data.len() == bands.ll.width * bands.ll.height;
    for (level, detail) in bands.levels.iter().enumerate() {
        let (low_x, low_y) = (x.size[level], y.size[level]);
        let (high_x, high_y) = (x.size[level + 1] - low_x, y.size[level + 1] - low_y);
        for (band, shape) in [
            (&detail.hl, (high_x, low_y)),
            (&detail.lh, (low_x, high_y)),
            (&detail.hh, (high_x, high_y)),
        ] {
            fits &= (band.width, band.height) == shape && band.data.len() == shape.0 * shape.1;
        }
    }
    if fits { Ok(()) } else { Err(inconsistent()) }
}

/// Merge the pyramid over the samples `footprint` needs, coarsest level
/// first, and return the window's samples.
fn reconstruct<S: Synthesis>(
    bands: Bands<S::Sample>,
    footprint: &Footprint,
) -> Result<Vec<S::Sample>> {
    check_shape(&bands, footprint)?;
    let Bands { ll, levels } = bands;
    let mut region = Region {
        columns: 0..ll.width,
        rows: 0..ll.height,
        data: ll.data,
    };
    for (level, detail) in levels.into_iter().enumerate() {
        region = merge_window::<S>(&region, &detail, footprint, level + 1);
    }
    // With no level to merge, the window is cut from the LL band.
    let (columns, rows) = (&footprint.x.need[0], &footprint.y.need[0]);
    if footprint.x.finest() == 0 && (region.columns != *columns || region.rows != *rows) {
        let mut out = Vec::with_capacity(columns.len() * rows.len());
        for row in rows.clone() {
            out.extend_from_slice(region.row(row, columns));
        }
        return Ok(out);
    }
    Ok(region.data)
}

/// Resolution `k` over the samples `footprint` needs of it, from `ll`, the
/// samples needed of resolution `k - 1`, and the level's detail bands. It
/// runs [`super::merge_level`]'s kernels in the same order, rows then
/// columns, over the footprint's span of the level, and keeps the samples
/// needed.
fn merge_window<S: Synthesis>(
    ll: &Region<S::Sample>,
    detail: &DetailBands<S::Sample>,
    footprint: &Footprint,
    k: usize,
) -> Region<S::Sample> {
    let (x, y) = (&footprint.x, &footprint.y);
    let (span_x, span_y) = (&x.span[k - 1], &y.span[k - 1]);
    let (cas_x, cas_y) = ((x.origin[k] & 1) as usize, (y.origin[k] & 1) as usize);
    let (low_x, high_x) = (&x.need[k - 1], &x.high[k - 1]);
    let (width, height) = (span_x.len(), span_y.len());
    let (columns, rows) = (x.need[k].clone(), y.need[k].clone());
    let mut out = vec![S::Sample::default(); columns.len() * rows.len()];
    if columns.is_empty() || rows.is_empty() {
        return Region {
            columns,
            rows,
            data: out,
        };
    }

    // The rows of the span, each lifted over the span's columns and stored
    // in image order. The run starts at sample `span_x.start`, so its parity
    // is that sample's.
    let mut grid = vec![S::Sample::default(); width * height];
    let run_x = (cas_x + span_x.start) & 1;
    let mut low = vec![S::Sample::default(); low_x.len()];
    let mut high = vec![S::Sample::default(); high_x.len()];
    for (line, row) in grid.chunks_exact_mut(width).zip(span_y.clone()) {
        let band = row / 2;
        let (low_src, high_src) = if (row + cas_y) % 2 == 0 {
            (ll.row(band, low_x), band_row(&detail.hl, band, high_x))
        } else {
            (
                band_row(&detail.lh, band, low_x),
                band_row(&detail.hh, band, high_x),
            )
        };
        low.copy_from_slice(low_src);
        high.copy_from_slice(high_src);
        let (even, odd) = if run_x == 0 {
            (&mut low[..], &mut high[..])
        } else {
            (&mut high[..], &mut low[..])
        };
        S::inverse_split(even, odd, run_x);
        interleave(line, even, odd);
    }

    // The columns needed, each lifted over the span's rows, in batches of
    // `LANES`, then the rest one at a time.
    let run_y = (cas_y + span_y.start) & 1;
    let first = columns.start - span_x.start;
    let top = rows.start - span_y.start;
    let count = columns.len();
    let full = count - count % LANES;
    let pass = Pass {
        grid: &grid,
        width,
        top,
        out_width: count,
        cas: run_y,
    };
    if full > 0 {
        let mut buf = vec![[S::Sample::default(); LANES]; height];
        for at in (0..full).step_by(LANES) {
            pass.columns::<S, LANES>(first + at, at, &mut buf, &mut out);
        }
    }
    if full < count {
        let mut buf = vec![[S::Sample::default(); 1]; height];
        for at in full..count {
            pass.columns::<S, 1>(first + at, at, &mut buf, &mut out);
        }
    }
    Region {
        columns,
        rows,
        data: out,
    }
}

/// The column pass of [`merge_window`]: the row-lifted span `grid`, `width`
/// wide, and where its columns go in the output, which keeps the rows from
/// `top` on, `out_width` wide.
struct Pass<'a, T> {
    grid: &'a [T],
    width: usize,
    top: usize,
    out_width: usize,
    cas: usize,
}

impl<T: Copy> Pass<'_, T> {
    /// Lift the `L` columns of the grid from `column` down the span, with
    /// the parity of its first row, and write the rows kept to the output's
    /// columns from `at`.
    fn columns<S: Synthesis<Sample = T>, const L: usize>(
        &self,
        column: usize,
        at: usize,
        buf: &mut [[T; L]],
        out: &mut [T],
    ) {
        for (lanes, src) in buf.iter_mut().zip(self.grid.chunks_exact(self.width)) {
            lanes.copy_from_slice(&src[column..column + L]);
        }
        S::inverse(buf, self.cas);
        for (dst, lanes) in out.chunks_exact_mut(self.out_width).zip(&buf[self.top..]) {
            dst[at..at + L].copy_from_slice(lanes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codestream;
    use crate::tier2;

    /// Each sample of a span is in exactly one of its low-pass and
    /// high-pass ranges, as sample `i / 2` of its band.
    #[test]
    fn low_and_high_split_a_span() {
        for cas in 0..2 {
            for n in 0..12 {
                for s in 0..=n {
                    for e in s..=n {
                        let (l, h) = (low(&(s..e), cas), high(&(s..e), cas));
                        let lows: Vec<usize> = (s..e).filter(|i| (i + cas) % 2 == 0).collect();
                        let highs: Vec<usize> = (s..e).filter(|i| (i + cas) % 2 == 1).collect();
                        assert_eq!(
                            l.collect::<Vec<_>>(),
                            lows.iter().map(|i| i / 2).collect::<Vec<_>>()
                        );
                        assert_eq!(
                            h.collect::<Vec<_>>(),
                            highs.iter().map(|i| i / 2).collect::<Vec<_>>()
                        );
                    }
                }
            }
        }
    }

    /// The coded data of tile `index` of `tests/fixtures/window/{name}.j2k`
    /// for component 0, handed to `check`.
    fn with_tile(name: &str, index: usize, check: impl Fn(&TileComponent, ComponentCoded<'_>)) {
        let bytes = std::fs::read(format!(
            "{}/tests/fixtures/window/{name}.j2k",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let parsed = codestream::parse_header(&bytes).unwrap();
        let cs = codestream::parse_tiles(&bytes, parsed).unwrap();
        let header = cs.tile_header(index).unwrap();
        let meter = tier2::Meter::new(u64::MAX);
        let mut coded =
            tier2::decode_packets_metered(&header, &cs.tiles[index], &meter, u64::MAX, 0, None)
                .unwrap();
        let rect = cs.header.siz.tile_component_rect(index as u32, 0).unwrap();
        let parts = TileComponent {
            rect,
            transform: header.components[0].coding.transform,
        };
        check(&parts, coded.components.swap_remove(0));
    }

    /// Component 0 of a tile: its rectangle and wavelet.
    struct TileComponent {
        rect: Rect,
        transform: Transform,
    }

    /// A window that covers the tile-component needs the whole-tile
    /// synthesis, and one that misses it needs no sample and no
    /// code-block.
    #[test]
    fn a_window_over_or_off_the_tile_component() {
        with_tile("tiled_53", 4, |parts, mut coded| {
            let rect = parts.rect;
            let over = Rect {
                x0: rect.x0 - 1,
                y0: rect.y0,
                x1: rect.x1 + 1,
                y1: rect.y1 + 7,
            };
            assert_eq!(
                Footprint::of(&coded, 0, rect, over, parts.transform).unwrap(),
                None
            );
            let off = Rect {
                x0: rect.x1,
                y0: rect.y0,
                x1: rect.x1 + 5,
                y1: rect.y1,
            };
            let footprint = Footprint::of(&coded, 0, rect, off, parts.transform)
                .unwrap()
                .unwrap();
            assert!(footprint.rect().is_empty());
            assert!(coded.resolutions.iter().any(|r| {
                r.subbands
                    .iter()
                    .any(|s| s.blocks.iter().any(|b| b.num_passes > 0))
            }));
            footprint.prune(&mut coded);
            for resolution in &coded.resolutions {
                for subband in &resolution.subbands {
                    assert!(subband.blocks.iter().all(|b| b.num_passes == 0));
                }
            }
        });
    }

    /// Subbands of another shape than the footprint's are refused, not
    /// sliced past their end.
    #[test]
    fn subbands_of_another_shape_are_inconsistent() {
        with_tile("tiled_53", 4, |parts, coded| {
            let rect = parts.rect;
            let part = Rect {
                x1: rect.x0 + 3,
                y1: rect.y0 + 3,
                ..rect
            };
            let footprint = Footprint::of(&coded, 0, rect, part, parts.transform)
                .unwrap()
                .unwrap();
            let band = |w: usize, h: usize| Band {
                origin: (0, 0),
                width: w,
                height: h,
                data: vec![0; w * h],
            };
            let bands = Bands {
                ll: band(2, 2),
                levels: vec![],
            };
            assert!(matches!(
                inverse_window(SubbandCoeffs::Reversible(bands), &footprint),
                Err(Error::Inconsistent(_))
            ));
            // More levels than the footprint has.
            let detail = || DetailBands {
                hl: band(1, 1),
                lh: band(1, 1),
                hh: band(1, 1),
            };
            let bands = Bands {
                ll: band(1, 1),
                levels: (0..40).map(|_| detail()).collect(),
            };
            assert!(matches!(
                inverse_window(SubbandCoeffs::Reversible(bands), &footprint),
                Err(Error::Inconsistent(_))
            ));
        });
    }
}
