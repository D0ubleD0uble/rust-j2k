//! Tile-component geometry for Tier-2 (ISO/IEC 15444-1 B.3–B.7).
//!
//! Turns a tile-component's rectangle on the reference grid into the
//! resolution → precinct → subband → code-block partition the packet parser
//! and progression walk read from. Every partition here is anchored at the
//! canvas origin: the resolution rectangle (B.5), the precinct grid (B.6), the
//! subband rectangles (B.5), and the code-block grid (B.7). The two decode
//! guards ([`MAX_PRECINCTS`], [`MAX_CODE_BLOCKS`]) live with the geometry they
//! bound, spent while it is built rather than after.

use crate::codestream::MainHeader;
use crate::codestream::markers::Rect;
use crate::{Error, Result};

use super::BandKind;

/// Ceiling on the number of precincts one tile may hold, across every component
/// and resolution. Each precinct costs a `PrecinctGeom` plus a tag-tree pair
/// (two `Vec`s) per subband, none of it bounded by the code-block or sample
/// budgets — a band empty in one axis carries no blocks while its resolution
/// still has a full column of precincts. 2^18 leaves any real encode far behind
/// (a 4096-square image at the 64×64 precincts JPIP favors needs ~5.5k) while
/// capping the allocation in the tens of megabytes. Enforced *while* the geometry
/// is built, before the precincts are allocated — see [`resolution_geoms`].
pub(crate) const MAX_PRECINCTS: usize = 1 << 18;

/// Ceiling on the number of code-blocks one tile may hold, across every
/// component, resolution, and subband. The sample budget bounds the decoded
/// buffers, but the per-block bookkeeping — `BlockState`, the tag trees, and the
/// eventual `CodeBlock`s, at most about 200 bytes a block — is driven by the code-block
/// *count*, which legal 4×4 blocks push toward samples/16: a sub-kilobyte header
/// could demand ~1 GiB of metadata. 2^19 clears every plausible real encode
/// (64×64 default blocks over one tile at the default 2^28-sample limit need
/// about 2^16.4) while capping
/// hostile geometry below 100 MiB. Enforced *while* the geometry is built, before
/// anything sized by a band's block count is allocated — see [`band_geom`].
pub(crate) const MAX_CODE_BLOCKS: usize = 1 << 19;

/// Geometry of one subband before its segments are parsed: orientation, origin,
/// sample dimensions, the code-block grid, and how the precincts divide that
/// grid up. Each block's band-relative position and size come from the grid
/// (see [`BandGeom::block_rects`]), so a band holds nothing per block.
pub(crate) struct BandGeom {
    pub(crate) kind: BandKind,
    pub(crate) origin: (u32, u32),
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) block_cols: usize,
    pub(crate) block_rows: usize,
    /// The band's sample bounds `[x0, x1) x [y0, y1)` on its own grid, as
    /// `(x0, x1, y0, y1)`.
    pub(crate) bounds: (i64, i64, i64, i64),
    /// The code-block grid's first column and row, counted from the canvas
    /// origin, and its exponents: block `(i, j)` of the band is cell
    /// `(first.0 + i, first.1 + j)` of a `2^exp.0 x 2^exp.1` grid, clipped to
    /// the band.
    pub(crate) first_block: (i64, i64),
    pub(crate) block_exp: (u32, u32),
    /// One entry per precinct of the enclosing resolution, in raster order — so
    /// `precincts.len()` is the same on every band of a resolution, even where
    /// the band is empty and each entry owns nothing.
    pub(crate) precincts: Vec<PrecinctGeom>,
}

/// One precinct's slice of one subband: the sub-grid of code-blocks it owns
/// (ISO/IEC 15444-1 B.7). It is a rectangle of the band's grid, so it is held
/// as where it starts and how wide and high it is.
///
/// The precinct partition and the code-block partition are both anchored at the
/// canvas origin, and a code-block is never larger than its precinct, so the
/// blocks of a precinct are a contiguous rectangle of the band's block grid and
/// every block belongs to exactly one precinct. `cols × rows` is therefore both
/// the block count and the dimensions of the two tag trees the packet header
/// runs over this precinct.
pub(crate) struct PrecinctGeom {
    pub(crate) cols: usize,
    pub(crate) rows: usize,
    /// The band index (row-major over the whole band) of the precinct's
    /// top-left code-block; 0 when it owns none.
    pub(crate) first: usize,
    /// The band's code-block columns: block `(x, y)` of the precinct is band
    /// block `first + y * stride + x`.
    pub(crate) stride: usize,
}

impl PrecinctGeom {
    /// The band index of block `(x, y)` of the precinct, `x < cols` and
    /// `y < rows`.
    pub(crate) fn block(&self, x: usize, y: usize) -> usize {
        self.first + y * self.stride + x
    }
}

impl BandGeom {
    /// The band's code-block count.
    pub(crate) fn block_count(&self) -> usize {
        self.block_cols * self.block_rows
    }

    /// `(x, y, width, height)` of each code-block, band-relative, row-major
    /// over the **whole band**: each cell of the code-block grid anchored at
    /// the canvas origin (ISO B.7), clipped to the band.
    pub(crate) fn block_rects(&self) -> impl Iterator<Item = (usize, usize, usize, usize)> + use<> {
        let (bx0, bx1, by0, by1) = self.bounds;
        let (first_col, first_row) = self.first_block;
        let cbw = 1i64 << self.block_exp.0;
        let cbh = 1i64 << self.block_exp.1;
        let cols = self.block_cols;
        (0..self.block_rows).flat_map(move |j| {
            let gy0 = (first_row + j as i64) * cbh;
            let cy0 = gy0.max(by0);
            let cy1 = (gy0 + cbh).min(by1);
            (0..cols).map(move |i| {
                let gx0 = (first_col + i as i64) * cbw;
                let cx0 = gx0.max(bx0);
                let cx1 = (gx0 + cbw).min(bx1);
                (
                    (cx0 - bx0) as usize,
                    (cy0 - by0) as usize,
                    (cx1 - cx0) as usize,
                    (cy1 - cy0) as usize,
                )
            })
        })
    }
}

/// One resolution level of a tile-component: its precinct grid and its subbands.
///
/// A packet exists per (layer, resolution, component, **precinct**), so the
/// precinct grid is what decides how many packets this resolution contributes.
/// A resolution whose rectangle is empty — `trx0 == trx1` or `try0 == try1` —
/// has **zero precincts** (ISO B.6) and so contributes *no* packet. Reading one
/// anyway would consume the next resolution's bytes and desynchronize the rest
/// of the tile.
///
/// Empty resolutions are not a curiosity: a tile-component only one sample wide
/// at an odd origin has `ceil(u0/2) == ceil(u1/2)`, so it vanishes one level up.
/// A single tile at the canvas origin can never produce one (its `trx0` is 0 and
/// its `trx1` at least 1), which is why this only matters once tiles exist.
/// OpenJPEG skips them the same way: `opj_pi_next_*` bounds its precinct loop by
/// `res->pw * res->ph`, which is zero here.
///
/// An empty resolution's subbands are empty too, since each lies within the
/// resolution on both axes, so they own no code-block and no packet reads
/// them. Their geometry is not built, and `bands` is empty: a tile of one
/// sample at an odd origin has 32 such resolutions per component. The
/// rectangles the inverse DWT still needs come from [`band_rects`].
pub(crate) struct ResolutionGeom {
    /// Precinct exponents `(PPx, PPy)` on **this resolution's** grid.
    pub(crate) ppx: u32,
    pub(crate) ppy: u32,
    /// The precinct grid: OpenJPEG's `res->pw` and `res->ph`.
    pub(crate) precincts_wide: usize,
    pub(crate) precincts_high: usize,
    /// The subbands in packet order, or none when the resolution holds no
    /// precinct.
    pub(crate) bands: Vec<BandGeom>,
}

impl ResolutionGeom {
    /// How many packets this resolution carries per (layer, component): one per
    /// precinct, and zero when the resolution is empty.
    pub(crate) fn precinct_count(&self) -> usize {
        self.precincts_wide * self.precincts_high
    }
}

/// `ceil(a / 2^exp)` for any integer `a` and `exp < 63`. The subband
/// formula's numerators can be negative and an arithmetic shift is a floor, so
/// this negates around it. Every divisor here is a power of two, and a shift
/// does not cost the tens of cycles an `i64` division does.
fn ceil_shr(a: i64, exp: u32) -> i64 {
    debug_assert!(exp < 63, "ceil_shr needs a shift below 63");
    -((-a) >> exp)
}

/// The subbands of resolution `r` of a tile-component on `rect` with `nl`
/// decomposition levels, in packet order: `NLLL` alone at resolution 0, else
/// `HL`, `LH`, `HH` at level `nb = nl - r + 1`. Each is its orientation and
/// its sample bounds `[x0, x1) x [y0, y1)` on its own grid (ISO B.5, Eq.
/// B-15), as `(kind, x0, x1, y0, y1)`; a bound pair can be equal, or crossed
/// by one, where the band is empty. `r` is at most `nl`, and `nl` at most 32.
pub(crate) fn band_rects(
    rect: Rect,
    nl: u32,
    r: u32,
) -> impl Iterator<Item = (BandKind, i64, i64, i64, i64)> {
    debug_assert!(r <= nl && nl <= 32, "resolution {r} of {nl} levels");
    let (tcx0, tcx1) = (i64::from(rect.x0), i64::from(rect.x1));
    let (tcy0, tcy1) = (i64::from(rect.y0), i64::from(rect.y1));
    let level = nl - r;
    let bands = if r == 0 {
        // The coarsest resolution carries only the NLLL band, and it *is* the
        // resolution rectangle.
        let ll = (
            BandKind::Ll,
            ceil_shr(tcx0, level),
            ceil_shr(tcx1, level),
            ceil_shr(tcy0, level),
            ceil_shr(tcy1, level),
        );
        [ll; 3]
    } else {
        let nb = level + 1;
        let half = 1i64 << (nb - 1);
        [
            (BandKind::Hl, 1, 0),
            (BandKind::Lh, 0, 1),
            (BandKind::Hh, 1, 1),
        ]
        .map(|(kind, xob, yob)| {
            (
                kind,
                ceil_shr(tcx0 - xob * half, nb),
                ceil_shr(tcx1 - xob * half, nb),
                ceil_shr(tcy0 - yob * half, nb),
                ceil_shr(tcy1 - yob * half, nb),
            )
        })
    };
    bands.into_iter().take(if r == 0 { 1 } else { 3 })
}

/// Compute the resolution → precinct → subband → code-block geometry for one
/// tile-component, coarsest resolution first (ISO B.5–B.7, Eq. B-14/B-15/B-16).
///
/// The bounds come from `tile`'s rect on *this component's* grid (B.3, Eq.
/// B-7/B-12): they divide by the component's sub-sampling, so two components of
/// one tile can yield different subband sizes, and they sit at the tile's own
/// offset, so two tiles of one component yield subbands at different origins.
/// Those origins are as load-bearing as the sizes — the inverse DWT reads its
/// interleave parity from them, and the assembly stage places the tile by them.
///
/// `precinct_budget` is the remaining precinct allowance across the whole tile;
/// this decrements it per resolution and errors before the allocation once it is
/// spent, so the [`MAX_PRECINCTS`] cap holds regardless of how a hostile header
/// splits the count between components. `block_budget` is the matching allowance
/// for code-blocks, spent inside [`band_geom`] for each band, so the
/// [`MAX_CODE_BLOCKS`] cap holds the same way before the walk allocates a
/// state per block.
pub(crate) fn resolution_geoms(
    header: &MainHeader,
    tile: u32,
    comp: usize,
    precinct_budget: &mut usize,
    block_budget: &mut usize,
) -> Result<Vec<ResolutionGeom>> {
    let siz = &header.siz;
    let cod = &header
        .components
        .get(comp)
        .ok_or_else(|| Error::Inconsistent(format!("no coding parameters for component {comp}")))?
        .coding;

    let nl = cod.decomposition_levels as i64;
    if nl > 32 {
        return Err(Error::Inconsistent(format!(
            "{nl} decomposition levels exceeds the 32-level maximum"
        )));
    }

    let rect = siz.tile_component_rect(tile, comp).ok_or_else(|| {
        Error::Inconsistent(format!("no tile {tile} or no component {comp} in SIZ"))
    })?;
    if rect.is_empty() {
        return Err(Error::Codestream(format!(
            "tile {tile} has an empty area on component {comp}"
        )));
    }
    let (tcx0, tcx1) = (i64::from(rect.x0), i64::from(rect.x1));
    let (tcy0, tcy1) = (i64::from(rect.y0), i64::from(rect.y1));

    // Code-block exponents (COD stores `log2(size) - 2`). The standard bounds
    // each at 2^10 and their sum at 2^12 (ISO Table A-18); reject anything
    // larger so the grid shifts below stay well-defined and a malformed COD is
    // a typed error, not a silently clamped mis-decode.
    let xcb = cod.code_block_width as u32 + 2;
    let ycb = cod.code_block_height as u32 + 2;
    if xcb > 10 || ycb > 10 || xcb + ycb > 12 {
        return Err(Error::Marker(format!(
            "code-block size 2^{xcb}×2^{ycb} exceeds the 2^10 / xcb+ycb≤12 limit"
        )));
    }

    let mut levels = Vec::with_capacity((nl + 1) as usize);
    for r in 0..=nl {
        // The resolution's own rectangle (ISO B.5, Eq. B-14). It is what the
        // precinct grid is cut from — not the bands', which can be empty while
        // the resolution is not.
        let level = nl - r;
        let shift = level as u32;
        let (trx0, trx1) = (ceil_shr(tcx0, shift), ceil_shr(tcx1, shift));
        let (try0, try1) = (ceil_shr(tcy0, shift), ceil_shr(tcy1, shift));

        // Precinct exponents are quoted on the resolution grid. The parser
        // guarantees a non-zero exponent above resolution 0, so the `- 1` below
        // is always in range.
        let (ppx, ppy) = cod.precinct(r as usize);
        debug_assert!(r == 0 || (ppx >= 1 && ppy >= 1));

        // The precinct grid over the resolution (ISO B.6, Eq. B-16), anchored at
        // the canvas origin like every other partition. `prc_span` returns the
        // count and the lattice point the first precinct starts on.
        let (precincts_wide, tl_prc_x) = prc_span(trx0, trx1, ppx);
        let (precincts_high, tl_prc_y) = prc_span(try0, try1, ppy);

        // Spend the tile's precinct budget before this resolution's precincts are
        // allocated below. `precincts_wide/high` are cheap products of the rect
        // and the exponents; the `PrecinctGeom`s that follow are not, so the cap
        // has to bite here rather than after the fact. `checked_mul` guards the
        // product itself, since a 2^0 precinct at resolution 0 makes each factor
        // as large as the resolution.
        let here = precincts_wide.checked_mul(precincts_high);
        match here.filter(|&n| n <= *precinct_budget) {
            Some(n) => *precinct_budget -= n,
            None => {
                return Err(Error::Limit(format!(
                    "tile precinct count exceeds the decode guard of {MAX_PRECINCTS}"
                )));
            }
        }

        // The *code-block group*: where that precinct lands on the subband grid.
        // At resolution 0 the band is the resolution, so the two coincide. Above
        // it the three bands sit on a grid one level coarser than the
        // resolution's, so the precinct halves — which is why a 2^0 precinct is
        // legal only at resolution 0, and why the code-block is capped one
        // exponent lower here (ISO B.6, Eq. B-17/B-18).
        let cbg = if r == 0 {
            Cbg {
                x0: tl_prc_x,
                y0: tl_prc_y,
                x_exp: ppx,
                y_exp: ppy,
                wide: precincts_wide,
                high: precincts_high,
            }
        } else {
            Cbg {
                x0: ceil_shr(tl_prc_x, 1),
                y0: ceil_shr(tl_prc_y, 1),
                x_exp: ppx - 1,
                y_exp: ppy - 1,
                wide: precincts_wide,
                high: precincts_high,
            }
        };
        // A code-block never outgrows the precinct that has to contain it
        // (ISO B.7): OpenJPEG's `cblkwidthexpn = min(tccp->cblkw, cbgwidthexpn)`.
        // With a maximal partition this caps at 2^15 / 2^14 and never bites; with
        // a 2^7 precinct at resolution 3 it shrinks a 2^6 block to 2^6, and a
        // 2^1 precinct shrinks it to a single sample.
        let (xcb, ycb) = (xcb.min(cbg.x_exp), ycb.min(cbg.y_exp));

        // An empty resolution has empty bands, which own no code-block, so its
        // geometry stops here (see [`ResolutionGeom`]).
        let mut bands = Vec::new();
        if precincts_wide * precincts_high > 0 {
            bands.reserve_exact(if r == 0 { 1 } else { 3 });
            for (kind, bx0, bx1, by0, by1) in band_rects(rect, nl as u32, r as u32) {
                bands.push(band_geom(
                    kind,
                    bx0,
                    bx1,
                    by0,
                    by1,
                    xcb,
                    ycb,
                    &cbg,
                    block_budget,
                )?);
            }
        }

        levels.push(ResolutionGeom {
            ppx,
            ppy,
            precincts_wide,
            precincts_high,
            bands,
        });
    }
    Ok(levels)
}

/// The precinct partition as it lands on the *subband* grid: the top-left of the
/// first code-block group, the group's exponents, and the grid's extent. One per
/// resolution, shared by its bands (ISO B.6).
struct Cbg {
    x0: i64,
    y0: i64,
    x_exp: u32,
    y_exp: u32,
    wide: usize,
    high: usize,
}

/// How many precincts of span `2^exp` cover `[lo, hi)`, and the lattice point the
/// first one starts on (ISO B.6, Eq. B-16). An empty span has zero precincts —
/// and so carries no packet — which is why this returns a count rather than
/// rounding up to one.
fn prc_span(lo: i64, hi: i64, exp: u32) -> (usize, i64) {
    if hi <= lo {
        return (0, 0);
    }
    let first = lo >> exp;
    ((ceil_shr(hi, exp) - first) as usize, first << exp)
}

/// Build one subband's geometry from its sample bounds `[bx0, bx1) × [by0, by1)`
/// and the effective code-block exponents, tiling it with the code-block grid
/// anchored at the canvas origin (ISO B.7), then grouping those blocks by the
/// precinct they fall in.
///
/// `block_budget` is the tile's remaining code-block allowance; this band's
/// `block_cols × block_rows` is spent against it before anything sized by the
/// block count is allocated (the walk's per-block state), so a hostile
/// geometry is rejected up front rather than after megabytes have been built.
#[allow(clippy::too_many_arguments)]
fn band_geom(
    kind: BandKind,
    bx0: i64,
    bx1: i64,
    by0: i64,
    by1: i64,
    xcb: u32,
    ycb: u32,
    cbg: &Cbg,
    block_budget: &mut usize,
) -> Result<BandGeom> {
    let width = (bx1 - bx0).max(0) as usize;
    let height = (by1 - by0).max(0) as usize;

    let (block_cols, first_col) = grid_span(bx0, bx1, xcb);
    let (block_rows, first_row) = grid_span(by0, by1, ycb);

    // Spend the tile's code-block budget before the walk allocates a state for
    // each of this band's blocks. `checked_mul` guards the product itself,
    // since a thin band under tiny blocks makes one factor as large as the
    // band.
    match block_cols
        .checked_mul(block_rows)
        .filter(|&n| n <= *block_budget)
    {
        Some(n) => *block_budget -= n,
        None => {
            return Err(Error::Limit(format!(
                "tile code-block count exceeds the decode guard of {MAX_CODE_BLOCKS}"
            )));
        }
    }

    // Cut the band's block grid up by precinct. A precinct's rectangle is its
    // code-block group clipped to the band (OpenJPEG's `prc->x0 = max(cbgxstart,
    // band->x0)`), and the block grid divides that rectangle evenly because the
    // block never outgrew the group above. So every block lands in exactly one
    // precinct, and a precinct the band does not reach simply owns none.
    let group_w = 1i64 << cbg.x_exp;
    let group_h = 1i64 << cbg.y_exp;
    let mut precincts = Vec::with_capacity(cbg.wide * cbg.high);
    for j in 0..cbg.high {
        let gy0 = cbg.y0 + j as i64 * group_h;
        let (py0, py1) = (gy0.max(by0), (gy0 + group_h).min(by1));
        for i in 0..cbg.wide {
            let gx0 = cbg.x0 + i as i64 * group_w;
            let (px0, px1) = (gx0.max(bx0), (gx0 + group_w).min(bx1));

            let (cols, col0) = grid_span(px0, px1, xcb);
            let (rows, row0) = grid_span(py0, py1, ycb);
            // A precinct the band does not reach owns no block, and its
            // `grid_span` start is a placeholder, not a cell of the band.
            let first = if cols * rows == 0 {
                0
            } else {
                (row0 - first_row) as usize * block_cols + (col0 - first_col) as usize
            };
            precincts.push(PrecinctGeom {
                cols,
                rows,
                first,
                stride: block_cols,
            });
        }
    }
    debug_assert_eq!(
        precincts.iter().map(|p| p.cols * p.rows).sum::<usize>(),
        block_cols * block_rows,
        "the precincts must partition the band's code-block grid"
    );

    Ok(BandGeom {
        kind,
        origin: (bx0.max(0) as u32, by0.max(0) as u32),
        width,
        height,
        block_cols,
        block_rows,
        bounds: (bx0, bx1, by0, by1),
        first_block: (first_col, first_row),
        block_exp: (xcb, ycb),
        precincts,
    })
}

/// Number of code-block grid cells spanning `[lo, hi)` and the index of the
/// first cell, with the grid anchored at multiples of `2^exp` from the origin.
fn grid_span(lo: i64, hi: i64, exp: u32) -> (usize, i64) {
    if hi <= lo {
        return (0, 0);
    }
    let first = lo >> exp;
    ((ceil_shr(hi, exp) - first) as usize, first)
}
