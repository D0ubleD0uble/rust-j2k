# Performance

This page compares the speed and peak memory of this decoder with OpenJPEG,
Grok, OpenJPH and hayro-jpeg2000 on the same files. It names the release it
measured, the machine, every version and build flag, and the commands, so you
can repeat it. Kakadu is left out because it is commercial.

Measured release: the rust-j2k 0.5.0 release candidate, commit `1f831a2`
(`Cargo.toml` still reads 0.4.0 there), and hayro-jpeg2000 0.5.0. This page
and the tools were added on top of that commit and do not change the
decoder. Two tables were measured again on commit `8284451`: the HTJ2K
table, for every decoder, and the incremental table. The other tables are
from `1f831a2`. Notes that quote the newer tables say so. The decoder changed
between the two commits: #448 made HTJ2K decoding faster (in process, 1.1x to 2.3x
depending on the file), which is why the rust-j2k HTJ2K times are lower than at `1f831a2`, and #452
and #454 changed the incremental decoder's limits.

## Results

All times are milliseconds of wall time per decode, the median of 7 runs.
Each cell is `time / peak MiB`; an in-process cell is the time alone. A
blank cell says why in the notes below the table. Command-line decoders are
timed as processes with their start-up and file I/O taken off (see
[Method](#method)); the in-process rows are the decode call alone.


### Part 1 files

| File | OpenJPEG 1 thread | OpenJPEG all cores | Grok 1 thread | Grok all cores | rust-j2k CLI | rust-j2k in process | hayro CLI | hayro in process |
| ---- | ----------------- | ------------------ | ------------- | -------------- | ------------ | ------------------- | --------- | ---------------- |
| `p0_07` | 1297.2 / 57.3 | 808.8 / 57.3 | 1238.2 / 47.5 | 291.4 / 49.0 | 1320.6 / 58.7 | 1301.2 | 1191.5 / 70.3 | 1201.7 |
| `p0_08` | 587.5 / 29.4 | 235.7 / 34.5 | 504.6 / 23.5 | 91.7 / 24.5 | 619.3 / 32.2 | 601.8 | 567.9 / 45.0 | 565.3 |
| `p0_05` | 175.5 / 18.5 | 31.6 / 18.8 | 176.0 / 17.5 | 26.4 / 18.8 | 217.3 / 18.5 | 204.3 | 201.5 / 33.9 | 203.1 |
| `p1_03` | 142.3 / 20.1 | 30.0 / 20.6 | 152.6 / 29.4 | 31.7 / 30.1 | 183.5 / 25.6 | 177.5 | 227.7 / 34.9 | 228.2 |
| `file7.jp2` | 170.7 / 14.0 | 65.0 / 14.9 | 151.6 / 22.1 | 39.5 / 22.8 | 185.7 / 11.8 | 182.2 | not converted to sRGB | not converted to sRGB |
| GRIB2-shaped | 105.5 / 10.4 | 47.1 / 10.9 | 86.8 / 17.2 | 30.7 / 18.2 | 104.4 / 9.5 | 100.6 | 98.7 / 17.0 | 96.6 |
| `regular_latlon` | 0.3 / 5.5 | 0.3 / 5.8 | 0.6 / 10.8 | 0.4 / 11.6 | 0.2 / 2.8 | 0.2 | 0.2 / 2.5 | 0.2 |
| tiled 2048, whole | 49.4 / 22.0 | 39.9 / 22.4 | 60.8 / 21.8 | 29.9 / 22.8 | 72.9 / 19.4 | 57.6 | 52.5 / 23.3 | 50.6 |

### Window and tile of the tiled file

Only decoders that can decode a region have these rows. OpenJPH reads HT
files only, and hayro-jpeg2000 has no region decode.

| Region | OpenJPEG 1 thread | OpenJPEG all cores | Grok 1 thread | Grok all cores | rust-j2k CLI | rust-j2k in process |
| ------ | ----------------- | ------------------ | ------------- | -------------- | ------------ | ------------------- |
| 256 x 256 window across four tiles | 3.0 / 6.7 | 1.6 / 6.4 | 2.1 / 11.7 | 2.2 / 12.7 | 3.1 / 3.5 | 2.6 |
| 64 x 64 window inside one tile | 1.1 / 6.2 | 1.4 / 6.1 | 0.4 / 11.0 | 1.3 / 11.8 | 1.3 / 3.2 | 1.0 |
| tile 27 | 1.9 / 6.1 | 1.5 / 6.0 | 1.9 / 11.2 | 0.8 / 11.5 | 2.1 / 3.4 | 2.9 |

### HTJ2K files

This table was measured on commit `8284451` at a one-minute load average of
0.44 to 0.57, with 7 runs per cell and the method below. Median and minimum
agree within 2% on the files over 20 ms and within 7% on `ds1_ht_02_b11`.
The in-process times of the small files vary from run to run. The two MIXED
in-process cells are medians of 7, like the others, but a full `run.py time`
run repeated three times gave medians of 2.2 to 5.0 ms for `ds0_hm_15_b8`
and 3.3 to 10.3 ms for `ds0_hm_06_b11`. The table keeps the first run, which
was the highest for both.
`perf-decode rust-j2k FILE --iters 30` on `ds0_hm_15_b8` took 1.4 ms per
decode. Read the MIXED cells as noise.

| File | OpenJPEG 1 thread | OpenJPEG all cores | Grok 1 thread | Grok all cores | OpenJPH | rust-j2k CLI | rust-j2k in process |
| ---- | ----------------- | ------------------ | ------------- | -------------- | ------- | ------------ | ------------------- |
| `ds0_ht_01_b11` | 0.5 / 5.7 | 0.4 / 5.8 | 0.0 / 10.8 | 0.3 / 11.2 | 0.0 / 5.5 | 0.5 / 3.0 | 0.4 |
| `hifi_ht1_02` | 3.4 / 5.8 | 4.1 / 5.8 | 1.9 / 11.9 | 3.0 / 15.4 | 1.0 / 5.5 | 2.1 / 3.2 | 2.9 |
| `ds1_ht_05_b11` | 23.1 / 11.3 | 88.0 / 11.4 | 84.8 / 47.7 | 41.6 / 88.7 | cannot decode | 27.0 / 6.4 | 25.5 |
| `ds1_ht_04_b9` | 30.6 / 9.8 | 48.5 / 9.7 | 14.3 / 22.5 | 22.2 / 85.2 | wrong image (tile-part `QCD`) | 10.8 / 6.9 | 7.8 |
| `ds0_hm_15_b8` (MIXED) | cannot decode | cannot decode | 2.3 / 11.3 | 0.3 / 11.5 | cannot decode | 2.1 / 3.0 | 5.0 |
| `ds0_hm_06_b11` (MIXED) | cannot decode | cannot decode | 4.0 / 11.7 | 4.6 / 17.6 | cannot decode | 3.9 / 3.7 | 10.3 |
| `ds1_ht_02_b11` | cannot decode | cannot decode | 7.2 / 13.5 | 4.2 / 14.2 | cannot decode | 16.8 / 8.0 | 13.2 |
| `ds0_ht_05_b11` | cannot decode | cannot decode | 21.1 / 18.5 | 10.0 / 19.6 | cannot decode | 57.6 / 18.7 | 49.7 |
| `ds0_ht_07_b16` | cannot decode | cannot decode | 117.3 / 65.2 | 48.6 / 71.7 | cannot decode | 245.7 / 59.7 | 203.6 |
| generated 2048 x 2048 gray | 40.3 / 22.4 | 20.1 / 23.8 | 17.2 / 20.0 | 9.2 / 22.1 | 13.2 / 7.0 | 50.8 / 27.8 | 45.0 |

hayro-jpeg2000 does not read HTJ2K: it stops on the first code-block style it
does not know, so it has no column here.

### Incremental decoder

`IncrementalDecoder` fed in 64 KB pieces, with `refine` called after each,
and `finish` at the end, against one `decode_with` of the same bytes. No
other decoder here has an incremental API, so there is nothing to compare it
with. Each row first checks that `finish` after the pieces gives the same
samples as one decode (it does, on both files). Times are medians of 9 runs,
measured on commit `8284451` at a one-minute load average of 0.51 to 0.66.
Each round times one decode and then one fed decode; the last column is the
range of that pair's ratio over the 9 rounds (`run.py incremental --runs 9`
prints it).

| File | One decode, ms | 64 KB pieces, ms | Ratio | Ratio over the 9 rounds |
| ---- | -------------- | ---------------- | ----- | ----------------------- |
| GRIB2-shaped | 98.2 | 245.2 | 2.50 | 2.48 to 2.59 |
| `ds0_ht_05_b11` | 50.1 | 155.4 | 3.10 | 2.87 to 3.19 |

The decoder tries to decode again only when the bytes fed have doubled since
the last try (see [`incremental-decoding.md`](incremental-decoding.md)), so
the extra time is explained by the number of tries, not the number of pieces.

### Files more than 2x slower than OpenJPEG

None, on the medians. On the files OpenJPEG decodes, the single-thread
command-line time of rust-j2k is between 0.35x and 1.5x of OpenJPEG's. The
slowest are the tiled 2048 file whole (1.48x), `p1_03` (1.29x) and, of the
HTJ2K files, the generated one (1.26x: 51 ms against 40 ms; in process,
45 ms). The HTJ2K ratios are from commit `8284451` and the others from
`1f831a2`. The HTJ2K files that decode in a few milliseconds have ratios
within the noise (see Small files below). On the files OpenJPEG cannot decode
there is no OpenJPEG time to compare with.

### Notes

- **Against OpenJPEG.** On the Part 1 files rust-j2k is within 10% of
  OpenJPEG on one thread for the large lossless files (`p0_07`, `p0_08`,
  `file7.jp2`, GRIB2-shaped), 24% to 48% slower on `p0_05`, `p1_03` and the
  tiled file. On the HTJ2K files (measured at `8284451`) it decodes
  `ds1_ht_04_b9` in a third of the time (10.8 ms against 30.6 ms) and
  `ds1_ht_05_b11` in 1.17x the time. Both are
  single-threaded in this comparison; OpenJPEG with all cores is 1.2x to
  5.5x faster on the larger Part 1 files and slower on the multi-tile HTJ2K
  files (`ds1_ht_05_b11`, `ds1_ht_04_b9`), where its threads cost more than
  they save.
- **Against Grok.** Grok on one thread is 1.1x to 1.2x as fast as rust-j2k on
  the Part 1 files. On four of the larger HTJ2K files (measured at `8284451`)
  it is faster: 2.3x on `ds1_ht_02_b11`, 2.7x on `ds0_ht_05_b11`, 2.1x on
  `ds0_ht_07_b16` and 3.0x on the generated file. On `ds1_ht_04_b9` rust-j2k
  is 1.3x faster, and on `ds1_ht_05_b11` it is 3.1x faster. This decoder's HT
  path is the slow part where it loses.
- **Against OpenJPH.** On the generated file (measured at `8284451`) OpenJPH is 3.8x faster than
  rust-j2k (13 ms against 51 ms), and uses a quarter of the memory. It
  decodes only two conformance files, both too small to time (below).
- **Against hayro-jpeg2000.** On the Part 1 files hayro-jpeg2000 is within
  about 25% of rust-j2k either way, except on the tiled file where it is
  1.39x faster: faster on `p0_07`, `p0_08`, `p0_05`, the GRIB2-shaped file
  and the tiled file, slower on `p1_03` (1.24x). Its output is 8-bit, so on
  the 12-bit files (`p0_07`, `p0_08`) and the 16-bit GRIB2-shaped file the
  work it does is not the same work. It uses more memory than rust-j2k on
  every file but the tiny `regular_latlon`.
- **Memory.** On the Part 1 files the peaks differ by up to 1.9x on the
  larger files, and no decoder is lowest throughout. On the tiny
  `regular_latlon` Grok is 3.9x rust-j2k (10.8 MiB against 2.8). rust-j2k is
  lowest on `file7.jp2` (11.8 MiB; OpenJPEG 14.0, Grok 22.1), the
  GRIB2-shaped file (9.5; 10.4 and 17.2) and the tiled file (19.4; 22.0 and
  21.8). It is level on `p0_05`. Grok is 19% lower on `p0_07` (47.5 MiB
  against 58.7) and 27% lower on `p0_08` (23.5 against 32.2), and OpenJPEG
  is 21% lower on `p1_03`. A window or one tile takes about half of
  OpenJPEG's peak and a quarter to a third of Grok's. On the HTJ2K
  conformance files (measured at `8284451`) rust-j2k's peak is the lowest or
  within 2% of Grok's: 18.7 MiB against 18.5 on `ds0_ht_05_b11`, 1.1x below on `ds0_ht_07_b16`,
  and 1.7x to 14x below on the rest. On the generated HTJ2K file it is the
  highest: 27.8 MiB against 22.4 for OpenJPEG, 20.0 for Grok and 7.0 for
  OpenJPH.
- **Small files.** For files that decode in a few milliseconds the
  command-line time is mostly process start-up, and subtracting the start-up
  run leaves noise: differences under about 2 ms there (`regular_latlon`,
  `ds0_ht_01_b11`, `hifi_ht1_02`, `ds0_hm_*`, the windows and tile 27) mean
  nothing, and a net of 0.0 means the decode was lost in the start-up run.
  The in-process column is the more reliable one for them, except for the
  MIXED rows, whose in-process cells vary from run to run.
- **Load.** The Part 1 timing ran at a one-minute load of 1.66 to 2.15. The
  HTJ2K table and the incremental table were measured again at 0.44 to 0.66. Median and
  minimum agree within a few percent on the files over 20 ms, which shows the
  runs were quiet.

## Output check

Every decoder's output was compared with the oracle before it was timed. A
decoder whose image was wrong has a note here and no time above.

The oracle is `opj_decompress` 2.5.0, writing PGX. OpenJPEG 2.5.0 does not
decode most of the HTJ2K conformance files: it refuses the MIXED code-block
set and fails on the others listed below. For those the oracle is the
ISO/IEC 15444-4 reference image that `tests/fixtures/conformance/ht/` holds,
and a lossy result counts as right inside the bound that manifest gives.
Lossless files must match exactly. For a lossy file against OpenJPEG, a peak
absolute error (PAE) of 5 or less counts as right, the loosest bound the
test suite uses for a lossy file (16-bit).

`PAE n` is the largest difference of any sample, over all components.

| File | Oracle | OpenJPEG | Grok | OpenJPH | rust-j2k | hayro-jpeg2000 |
| ---- | ------ | -------- | ---- | ------- | -------- | -------------- |
| `p0_07` | OpenJPEG | exact | exact | n/a | exact | exact |
| `p0_08` | OpenJPEG | exact | exact | n/a | exact | exact |
| `p0_05` | OpenJPEG | exact | PAE 1 | n/a | PAE 1 | PAE 1, but see notes |
| `p1_03` | OpenJPEG | exact | PAE 1 | n/a | PAE 1 | PAE 1, but see notes |
| `file7.jp2` | OpenJPEG | exact | exact | n/a | exact | PAE 24574: not converted to sRGB (`colr` METH 2) |
| GRIB2-shaped | OpenJPEG | exact | exact | n/a | exact | exact |
| `regular_latlon` | OpenJPEG | exact | exact | n/a | exact | exact |
| tiled 2048, whole and the three regions | OpenJPEG | exact | exact | n/a | exact | exact (whole only) |
| `ds0_ht_01_b11` | OpenJPEG | exact | exact | exact | exact | cannot decode |
| `hifi_ht1_02` | OpenJPEG | exact | PAE 1 | PAE 1 | PAE 1 | cannot decode |
| `ds1_ht_05_b11` | OpenJPEG | exact | PAE 1 | cannot decode (2 layers) | PAE 1 | cannot decode |
| `ds1_ht_04_b9` | OpenJPEG | exact | PAE 1 | wrong image, PAE 956 | PAE 1 | cannot decode |
| `ds0_hm_15_b8` | ISO reference | cannot decode (MIXED) | exact | cannot decode (MIXED) | exact | cannot decode |
| `ds0_hm_06_b11` | ISO reference | cannot decode (MIXED) | PAE 367, in bound | cannot decode (MIXED) | PAE 367, in bound | cannot decode |
| `ds1_ht_02_b11` | ISO reference | cannot decode | PAE 3, in bound | cannot decode (2+ layers) | PAE 2, in bound | cannot decode |
| `ds0_ht_05_b11` | ISO reference | cannot decode | PAE 2, in bound | cannot decode (2+ layers) | PAE 2, in bound | cannot decode |
| `ds0_ht_07_b16` | ISO reference | cannot decode | exact | cannot decode (2+ layers) | exact | cannot decode |
| generated 2048 x 2048 gray | OpenJPEG | exact | exact | exact | exact | cannot decode |

What the notes in the table mean:

- OpenJPH 0.33.0 decodes one quality layer and ignores a `QCD` marker inside a
  tile part. `ds1_ht_04_b9` has such markers, so OpenJPH decodes it to a wrong
  image and is not timed on it. It also does not read the MIXED code-block
  style, and its raw output cannot be a color-transformed 3-component file,
  so it writes PPM for those.
- hayro-jpeg2000 gives each component back at the full image size, so for the
  two files with sub-sampled chroma (`p0_05`, `p1_03`) only the two full-size
  components are compared. It writes every component unsigned, so the signed
  12-bit files were compared after adding 2048 to the oracle. It does not
  convert `file7.jp2` to sRGB, which is why it differs from OpenJPEG's output
  there. It is not timed on that file.
- hayro-jpeg2000's 8-bit output (`data_u8`) is what it timed here. On the
  12-bit and 16-bit files that is a reduced-depth result, and its CLI output
  is smaller than the others for that reason. The check above uses its
  components at their coded depth, before the 8-bit step.
- `p0_05` and `p1_03` have components that use the 9/7 filter, so decoders
  that round differently differ by at most 1 sample value there.

## Files

| File | Size | What it is |
| ---- | ---- | ---------- |
| `p0_07` | 7.5 MiB | conformance, 2048 x 2048, 3 components, 12-bit signed, 5/3 lossless, 16 x 16 tiles, 8 layers |
| `p0_08` | 3.6 MiB | conformance, 513 x 3072, 3 components, 12-bit signed, 5/3 lossless, 30 layers |
| `p0_05` | 1.2 MiB | conformance, 1024 x 1024, 4 components (two sub-sampled), 8-bit, 5/3 and 9/7 components, 7 layers |
| `p1_03` | 1.3 MiB | conformance, 1024 x 1024, 4 components (two sub-sampled), 8-bit, 5/3 and 9/7 components, 10 layers |
| `file7.jp2` | 1.3 MiB | conformance JP2, 480 x 640, 3 components, 16-bit, color transform, needs sRGB conversion |
| GRIB2-shaped | 676 KiB | 1440 x 721, 1 component, 16-bit, 5/3 lossless, the shape of a GRIB2 grid |
| `regular_latlon` | 1 KiB | 16 x 31, 1 component, 16-bit |
| tiled 2048 | 195 KiB | 2048 x 2048, 1 component, 8-bit, 5/3 lossless, 8 x 8 tiles of 256 x 256 |
| HTJ2K files | 8 KiB to 8.5 MiB | ISO/IEC 15444-4 HTJ2K conformance codestreams; see below |

The HTJ2K files, from the small to the large. All are HT-only except the two
MIXED ones. They cover lossless and lossy, 4 to 12 bits, one to four
components, one to 16 x 16 tiles and one to 19 layers.

| File | Size | Image | Wavelet | Code-block set |
| ---- | ---- | ----- | ------- | -------------- |
| `ds0_ht_01_b11` | 8 KiB | 128 x 128, 1 comp, 8-bit | 5/3, lossless | HT |
| `hifi_ht1_02` | 30 KiB | 128 x 128, 3 comps, 12-bit | 9/7, lossy | HT |
| `ds1_ht_05_b11` | 290 KiB | 512 x 512, 3 comps, 8-bit, 15 x 15 tiles, 2 layers | 9/7, lossy | HT |
| `ds1_ht_04_b9` | 45 KiB | 1024 x 1024, 1 comp, 12-bit, 8 x 8 tiles | 9/7, lossy | HT |
| `ds0_hm_15_b8` | 17 KiB | 256 x 256, 1 comp, 4-bit, 2 x 2 tiles | 5/3, lossless | MIXED |
| `ds0_hm_06_b11` | 21 KiB | 513 x 129, 4 comps, 12-bit | 9/7, lossy | MIXED |
| `ds1_ht_02_b11` | 259 KiB | 640 x 480, 3 comps, 8-bit, 19 layers | 9/7, lossy | HT |
| `ds0_ht_05_b11` | 1.3 MiB | 1024 x 1024, 4 comps, 8-bit, 7 layers | 9/7, lossy | HT |
| `ds0_ht_07_b16` | 8.5 MiB | 2048 x 2048, 3 comps, 12-bit, 16 x 16 tiles, 8 layers | 5/3, lossless | HT |

One more HTJ2K file is generated, not a conformance file, so that OpenJPH has
a large file it can decode (it decodes one layer only). `run.py` makes it
each run from the tiled 2048 file, with the 8-bit gray image decoded to PGM
and compressed losslessly, in one layer, by OpenJPH 0.33.0:

```sh
opj_decompress -i tests/fixtures/bench/tiled_2048_gray.j2k -o gray.pgm
ojph_compress -i gray.pgm -o ht_gen_2048_gray.j2c -reversible true -num_decomps 5
```

Its oracle is OpenJPEG, which decodes it. The row is marked generated in the
tables.

Where each file lives is in `tests/fixtures/README.md` and the conformance
manifests; none of them is in the published crate.

## Method

- Every decoder is a release build. Each cell is the median of 7 runs, with
  the decoders interleaved: one run of every decoder on a file, then the next
  round. Wall time comes from the clock around the process; peak resident
  memory comes from `/usr/bin/time -f %M`.
- The command-line decoders write their output to a RAM-backed directory
  (`/dev/shm`), so file writes are a memory copy, not a disk write. OpenJPEG
  and Grok write PGX, one file per component. OpenJPH writes PGM, PPM or raw
  planar. The rust-j2k program writes PGX like OpenJPEG. The hayro-jpeg2000
  program writes the interleaved 8-bit bytes its API returns.
- The start-up of each command-line decoder is measured the same way, on an
  8 x 8 gray file (made with `opj_compress` for all but OpenJPH, and with
  `ojph_compress` for OpenJPH), in the same round, and subtracted. That
  removes process start-up, library loading and the cost of opening files, so
  what is left is the decode and writing the output.
- rust-j2k and hayro-jpeg2000 are timed twice: as a command-line program
  (comparable with the others) and in process. The in-process time is
  one decode after a warm-up decode, with the file already in memory. The peak
  memory is for the command-line run, so it includes the process and the file
  read.
- The decoders that can use several threads are timed on one thread and on
  all cores. OpenJPEG runs with no `-threads` option (one thread) and with
  `-threads ALL_CPUS`. Grok runs with `-H 1` and `-H 16`. The 16 is the
  number of logical CPUs, which is what `ALL_CPUS` means here, so both get
  the same count.
  rust-j2k, hayro-jpeg2000 and OpenJPH decode on one thread; they have no
  thread option.
- Each output is checked against the oracle first (see [Output
  check](#output-check)). A decoder with a wrong image gets no time.
- This crate decodes with default `DecodeOptions`, except that JP2 files ask
  for sRGB output, since `opj_decompress` and Grok convert to sRGB by default.
  A window or tile is a `DecodeOptions::region` of that kind.

## Machine

CPU: Intel Core i7-10700KF at 3.8 GHz, 8 cores and 16 threads. The CPU
frequency governor was `powersave` (the `intel_pstate` driver); it was not
changed, and it affects absolute times. The one-minute load average was 1.66
when the timing started and 2.15 when it ended. The machine is shared, so
other jobs ran. The HTJ2K and incremental tables were measured again later, on
the same machine, at a load of 0.44 to 0.66 (see their notes).

## Decoders and builds

| Decoder | Version | Source | Build |
| ------- | ------- | ------ | ----- |
| rust-j2k | 0.5.0 release candidate, commit `1f831a2` (`Cargo.toml` reads 0.4.0); the HTJ2K and incremental tables, commit `8284451` (`Cargo.toml` reads 0.5.0) | this repository | `cargo build --release`, rustc 1.100.0 for both commits, default release profile |
| OpenJPEG | 2.5.0 (`opj_decompress`) | the Ubuntu 24.04 package, `2.5.0-2ubuntu0.5` | as the distribution built it |
| Grok | 20.4.25, commit `46ab34de` | tag `v20.4.25` of GrokImageCompression/grok, built from source | CMake, `Release` (`-O3 -DNDEBUG`), g++ 13.3, shared libraries, no sudo; LittleCMS bundled, the other libraries from the system |
| OpenJPH | 0.33.0, commit `345a8bcd` | tag `0.33.0` of aous72/OpenJPH, built from source | CMake, `Release` (`-O3 -DNDEBUG`), g++ 13.3 |
| hayro-jpeg2000 | 0.5.0 | crates.io | `cargo build --release`, features `std` and `simd`, rustc 1.100.0, as for rust-j2k |

## Reproduce

The programs and scripts are in `tools/perf/`. The directory is its own Cargo
package with its own lockfile, outside the crate, so the crate gains no
dependency and the published package does not include it.

Build Grok and OpenJPH from source, with no root access:

```sh
git clone --depth 1 --branch v20.4.25 https://github.com/GrokImageCompression/grok
git -C grok submodule update --init --depth 1 \
  src/include/CLI11 src/lib/core/highway src/include/spdlog src/include/taskflow
cmake -S grok -B grok-build -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$HOME/.local/opt/grok" \
  -DCMAKE_INSTALL_RPATH="$HOME/.local/opt/grok/lib" \
  -DBUILD_SHARED_LIBS=ON -DGRK_BUILD_CORE_EXAMPLES=OFF \
  -DGRK_BUILD_CORE_SWIG_BINDINGS=OFF -DBUILD_TESTING=OFF \
  -DGRK_BUILD_JPEG=OFF -DGRK_BUILD_LIBPNG=OFF -DGRK_BUILD_LIBTIFF=OFF
nice -n 19 ninja -C grok-build -j2 install

git clone --depth 1 --branch 0.33.0 https://github.com/aous72/OpenJPH
cmake -S OpenJPH -B ojph-build -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$HOME/.local/opt/openjph" \
  -DCMAKE_INSTALL_RPATH="$HOME/.local/opt/openjph/lib" -DOJPH_BUILD_TESTS=OFF
nice -n 19 ninja -C ojph-build -j2 install
```

Build the comparison program, which holds the rust-j2k and hayro-jpeg2000
decoders, and run the check and the timings. The script needs `python3` with
`numpy`, GNU `time` at `/usr/bin/time`, and `opj_decompress` and
`opj_compress` 2.5.0 on `PATH`.

```sh
cd tools/perf
cargo build --release
export PERF_DECODE=$PWD/target/release/perf-decode
export GROK=$HOME/.local/opt/grok/bin/grk_decompress
export OJPH=$HOME/.local/opt/openjph/bin/ojph_expand
export OJPH_COMPRESS=$HOME/.local/opt/openjph/bin/ojph_compress
python3 run.py check                  # the output check
python3 run.py time --runs 7          # the speed and memory tables
python3 run.py incremental --runs 9   # the incremental table
```

`run.py time` and `run.py check` take `--only SUBSTRING` to run the files whose
name contains it. `perf-decode` also runs by hand, for example
`perf-decode rust-j2k FILE --iters 10` for in-process times, and
`perf-decode hayro FILE --out PREFIX` to write the components. Its header
comment lists every flag.
