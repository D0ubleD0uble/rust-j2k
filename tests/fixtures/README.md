# Conformance fixtures

Each fixture is a raw JPEG 2000 codestream (`<name>.j2k`, Annex A, no JP2 boxes)
paired with a sibling oracle snapshot `<name>.expected.json`. The conformance
harness decodes the codestream and compares its output against the snapshot. The
snapshot is what a trusted reference decoder (OpenJPEG's `opj_decompress`, or
eccodes for GRIB2-sourced files) produced, committed so the corpus is graded
offline without the reference decoder installed.

## The corpus

Every fixture is the exact subset Phase 1 decodes: one component, one tile, one
quality layer, LRCP, no precincts. Each is within ISO/IEC 15444 Part 1 and
squarely inside the Phase 1 scope. The first three are GRIB2 §5.40 (`grid_jpeg`)
codestreams; the 9/7 one is an OpenJPEG re-encode of a GRIB2 grid (see the note
below the table).

| Fixture | Grid | Depth | Wavelet | Tolerance |
| ------- | ---- | ----- | ------- | --------- |
| `jpeg2000_regular_latlon`  | 16×31 | 16-bit unsigned | 5/3 reversible | `exact` |
| `eta_lambert_lossless`     | 93×65 | 13-bit unsigned | 5/3 reversible | `exact` |
| `regular_latlon_lossy`     | 16×31 | 16-bit unsigned | 5/3 reversible (rate-truncated) | `exact` |
| `eta_lambert_irreversible` | 93×65 | 13-bit unsigned | 9/7 irreversible | `absolute`, 2.0 |

The three 5/3 fixtures gate bit-exact: 5/3 is exact integer lifting, so any
difference from OpenJPEG is a bug. That includes `regular_latlon_lossy`, whose
codestream is lossy by rate truncation (fewer bit-planes) but whose decode is
still deterministic integer math. Only `eta_lambert_irreversible` uses the 9/7
float lifting, so it gates within a per-sample absolute bound: how far our 9/7
reconstruction may drift from OpenJPEG's, both decoding the same codestream, not
a compression budget. Measured drift is ≤ 1; the 2.0 bound leaves headroom.

No operational producer ships lossy 9/7 JPEG 2000 in GRIB2. HRRR and NDFD are
complex-packed, ECMWF is CCSDS, and eccodes' `grid_jpeg` encoder always uses the
reversible 5/3 transform regardless of `typeOfCompressionUsed`. So the only way
to exercise the irreversible path against the oracle is to re-encode a real grid
with OpenJPEG's irreversible (`-I`) mode; `eta_lambert_irreversible` is that
re-encode.

### Provenance and license

All four derive from small public GRIB2 fixtures already vendored in the sibling
`fieldglass` repo. This keeps the corpus tiny and regenerable offline, with no
network fetch and no large operational grids.

- `jpeg2000_regular_latlon` — the codestream from `fieldglass`'s
  `jpeg2000_regular_latlon.grib2`, itself an eccodes `grid_jpeg` repack of ECMWF's
  `regular_latlon_surface.grib2` test sample. eccodes and its samples are
  Apache-2.0.
- `eta_lambert_lossless` — eccodes `grid_jpeg` repack of `fieldglass`'s
  `eta_lambert_msg0.grib2` (first message of NOAA Eta output via the pygrib sample
  corpus). NOAA model output is U.S. government work in the public domain.
- `regular_latlon_lossy` — eccodes lossy (`typeOfCompressionUsed=1`) `grid_jpeg`
  repack of ECMWF's `regular_latlon_surface.grib2`. Still a 5/3 codestream: that
  eccodes option truncates the bit-stream, it does not switch to the 9/7
  transform. Apache-2.0.
- `eta_lambert_irreversible` — the samples of `eta_lambert_lossless` re-encoded
  with OpenJPEG's irreversible 9/7 transform (`opj_compress -I`). Same NOAA Eta
  grid, public domain; only the codestream's wavelet differs.

All sources are Apache-2.0 or public domain, compatible with this repo.

### Regenerating the corpus

The oracle toolchain (`scripts/install-oracle-tools.sh`; eccodes + OpenJPEG) and
the sibling `fieldglass` checkout are the only inputs. `FG` points at
`fieldglass/crates/fieldglass-grib2/tests/fixtures`.

```sh
FG=../fieldglass/crates/fieldglass-grib2/tests/fixtures

# 1. real lossless 5/3: extract the codestream straight from the §5.40 message.
scripts/extract-grib2-codestream.py "$FG/jpeg2000_regular_latlon.grib2" \
    -o tests/fixtures/jpeg2000_regular_latlon.j2k
scripts/gen-oracle.sh tests/fixtures/jpeg2000_regular_latlon.j2k \
    --source "fieldglass jpeg2000_regular_latlon.grib2 (eccodes grid_jpeg repack of ECMWF regular_latlon_surface.grib2)"

# 2. lossless 5/3 at a second geometry: repack a different base to grid_jpeg.
grib_set -r -s packingType=grid_jpeg "$FG/eta_lambert_msg0.grib2" /tmp/eta.grib2
scripts/extract-grib2-codestream.py /tmp/eta.grib2 -o tests/fixtures/eta_lambert_lossless.j2k
scripts/gen-oracle.sh tests/fixtures/eta_lambert_lossless.j2k \
    --source "eccodes grid_jpeg repack of fieldglass eta_lambert_msg0.grib2 (NOAA Eta, Lambert 93x65)"

# 3. lossy 5/3: repack with typeOfCompressionUsed=1. This truncates the
#    bit-stream (lossy codestream) but stays on the reversible 5/3 transform,
#    so the decode is still bit-exact against the oracle.
grib_set -r -s packingType=grid_jpeg,typeOfCompressionUsed=1,targetCompressionRatio=20 \
    "$FG/regular_latlon_surface.grib2" /tmp/lossy.grib2
scripts/extract-grib2-codestream.py /tmp/lossy.grib2 -o tests/fixtures/regular_latlon_lossy.j2k
scripts/gen-oracle.sh tests/fixtures/regular_latlon_lossy.j2k \
    --source "eccodes grid_jpeg lossy (typeOfCompressionUsed=1, ratio 20) repack of ECMWF regular_latlon_surface.grib2" \
    --notes "5/3 reversible (qmfbid=1) made lossy by rate truncation: eccodes' grid_jpeg encoder always uses the reversible 5/3 transform, never 9/7. The inverse is exact integer lifting, so our decode agrees with OpenJPEG bit-exactly."

# 4. irreversible 9/7: no GRIB2 producer ships lossy 9/7, so re-encode the Eta
#    grid's samples (from step 2) with OpenJPEG's irreversible transform.
opj_decompress -i tests/fixtures/eta_lambert_lossless.j2k -o /tmp/eta.pgx
opj_compress -i /tmp/eta_0.pgx -o tests/fixtures/eta_lambert_irreversible.j2k -I
scripts/gen-oracle.sh tests/fixtures/eta_lambert_irreversible.j2k \
    --tolerance 2.0 \
    --source "9/7 irreversible re-encode of the committed eta_lambert_lossless.j2k (NOAA Eta, Lambert 93x65, 13-bit) via opj_compress -I" \
    --notes "Synthetic 9/7: no operational producer ships lossy 9/7 JPEG2000 (HRRR/NDFD are complex-packed, ECMWF is CCSDS), so the irreversible codestream is produced by OpenJPEG from a real grid. Bound is our-vs-OpenJPEG reconstruction tolerance, not the lossy compression error."
```

Each `gen-oracle.sh` run records the exact `opj_decompress` command it ran into the
snapshot's `provenance.oracle_command`, so the corpus regrows from these inputs
alone. `cargo test` never invokes any of this — it reads the committed snapshots.

## `<name>.expected.json` schema

The schema is the `Expected` type in `tests/support/mod.rs`; it deserializes the
fields below. Unknown fields are rejected, so a typo'd key fails the parse rather
than silently dropping data. Keep this document and that type in step.

| Field        | Type     | Meaning                                                        |
| ------------ | -------- | ------------------------------------------------------------- |
| `geometry`   | object   | Sample-grid shape (below).                                    |
| `tolerance`  | object   | How close our decode must be to pass (below).                 |
| `samples`    | int[]    | `width * height` reference samples, row-major.                |
| `provenance` | object   | Where the fixture came from and how to regenerate (below).    |

### `geometry`

| Field       | Type   | Meaning                                            |
| ----------- | ------ | -------------------------------------------------- |
| `width`     | u32    | Component width in samples.                        |
| `height`    | u32    | Component height in samples.                       |
| `bit_depth` | u8     | Bits per sample as declared in SIZ (1–32).         |
| `signed`    | bool   | Whether samples are signed (SIZ component sign).   |

### `tolerance`

Tagged by `mode`, so both arms read the same in JSON:

- **Reversible (5/3, lossless)** — bit-exact agreement:

  ```json
  "tolerance": { "mode": "exact" }
  ```

- **Irreversible (9/7, lossy)** — each sample within `max_abs_error` (absolute):

  ```json
  "tolerance": { "mode": "absolute", "max_abs_error": 1.0 }
  ```

### `provenance`

| Field            | Type            | Meaning                                                  |
| ---------------- | --------------- | -------------------------------------------------------- |
| `source`         | string          | Where the codestream came from.                          |
| `oracle_command` | string          | Exact command that regenerates this snapshot.            |
| `notes`          | string, optional | Free-form note (how the source was obtained, caveats).  |

## Example

```json
{
  "geometry": { "width": 2, "height": 2, "bit_depth": 16, "signed": false },
  "tolerance": { "mode": "exact" },
  "samples": [0, 1, 2, 3],
  "provenance": {
    "source": "fieldglass/jpeg2000_regular_latlon.grib2",
    "oracle_command": "opj_decompress -i sample.j2k -o sample.pgx"
  }
}
```
