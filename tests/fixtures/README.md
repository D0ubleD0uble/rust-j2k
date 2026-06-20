# Conformance fixtures

Each fixture is a raw JPEG 2000 codestream (`<name>.j2k`, Annex A, no JP2 boxes)
paired with a sibling oracle snapshot `<name>.expected.json`. The conformance
harness decodes the codestream and compares its output against the snapshot. The
snapshot is what a trusted reference decoder (OpenJPEG's `opj_decompress`, or
eccodes for GRIB2-sourced files) produced, committed so the corpus is graded
offline without the reference decoder installed.

## The corpus

Every fixture is a GRIB2 §5.40 (`grid_jpeg`) codestream, the exact subset Phase 1
decodes: one component, one tile, one quality layer, LRCP, no precincts. Each is
within ISO/IEC 15444 Part 1 and squarely inside the Phase 1 scope.

| Fixture | Grid | Depth | Wavelet | Tolerance |
| ------- | ---- | ----- | ------- | --------- |
| `jpeg2000_regular_latlon` | 16×31 | 16-bit unsigned | 5/3 reversible | `exact` |
| `eta_lambert_lossless`    | 93×65 | 13-bit unsigned | 5/3 reversible | `exact` |
| `regular_latlon_lossy`    | 16×31 | 16-bit unsigned | 9/7 irreversible | `absolute`, 2.0 |

The two reversible fixtures gate bit-exact; the irreversible one gates within a
per-sample absolute bound. That bound is how far our 9/7 reconstruction may drift
from OpenJPEG's (both decode the same lossy codestream — the lossy compression
error is already baked into the committed samples), not a compression budget.

### Provenance and license

All three derive from small public GRIB2 fixtures already vendored in the sibling
`fieldglass` repo, repacked to `grid_jpeg` with eccodes — the same method that
produced `fieldglass`'s own `jpeg2000_regular_latlon.grib2`. This keeps the corpus
tiny and regenerable offline, with no network fetch and no large operational grids.

- `jpeg2000_regular_latlon` — the codestream from `fieldglass`'s
  `jpeg2000_regular_latlon.grib2`, itself an eccodes `grid_jpeg` repack of ECMWF's
  `regular_latlon_surface.grib2` test sample. eccodes and its samples are
  Apache-2.0.
- `eta_lambert_lossless` — eccodes `grid_jpeg` repack of `fieldglass`'s
  `eta_lambert_msg0.grib2` (first message of NOAA Eta output via the pygrib sample
  corpus). NOAA model output is U.S. government work in the public domain.
- `regular_latlon_lossy` — eccodes lossy (`typeOfCompressionUsed=1`) `grid_jpeg`
  repack of ECMWF's `regular_latlon_surface.grib2`. Apache-2.0.

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

# 3. lossy 9/7: repack with typeOfCompressionUsed=1.
grib_set -r -s packingType=grid_jpeg,typeOfCompressionUsed=1,targetCompressionRatio=20 \
    "$FG/regular_latlon_surface.grib2" /tmp/lossy.grib2
scripts/extract-grib2-codestream.py /tmp/lossy.grib2 -o tests/fixtures/regular_latlon_lossy.j2k
scripts/gen-oracle.sh tests/fixtures/regular_latlon_lossy.j2k \
    --tolerance 2.0 \
    --source "eccodes grid_jpeg lossy (typeOfCompressionUsed=1, ratio 20) repack of ECMWF regular_latlon_surface.grib2" \
    --notes "9/7 irreversible path; bound is our-vs-OpenJPEG reconstruction tolerance, not the lossy compression error"
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
