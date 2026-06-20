# Conformance fixtures

Each fixture is a raw JPEG 2000 codestream (`<name>.j2k`, Annex A, no JP2 boxes)
paired with a sibling oracle snapshot `<name>.expected.json`. The conformance
harness decodes the codestream and compares its output against the snapshot. The
snapshot is what a trusted reference decoder (OpenJPEG's `opj_decompress`, or
eccodes for GRIB2-sourced files) produced, committed so the corpus is graded
offline without the reference decoder installed.

No codestreams live here yet — they arrive with the seed corpus. This file
defines the snapshot schema so they can be added against a fixed format.

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
