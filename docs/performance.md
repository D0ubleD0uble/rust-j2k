# Performance

This page compares the speed and peak memory of this decoder with OpenJPEG,
Grok, OpenJPH and hayro-jpeg2000 on the same files. It names the release it
measured, the machine, every version and build flag, and the commands, so you
can repeat it. Kakadu is left out because it is commercial.

Measured release: rust-j2k RELEASE (commit COMMIT).

## Results

All times are milliseconds of wall time per decode, the median of RUNS runs.
Each cell is `time / peak MiB`. A blank cell says why in the notes below the
table. Command-line decoders are timed as processes with their start-up and
file I/O taken off (see [Method](#method)); the in-process rows are the decode
call alone.

TO FILL: the tables below are empty until the timing run.

### Part 1 files

| File | OpenJPEG 1 thread | OpenJPEG all cores | Grok 1 thread | Grok all cores | rust-j2k CLI | rust-j2k in process | hayro CLI | hayro in process |
| ---- | ----------------- | ------------------ | ------------- | -------------- | ------------ | ------------------- | --------- | ---------------- |
| `p0_07` | | | | | | | | |
| `p0_08` | | | | | | | | |
| `p0_05` | | | | | | | | |
| `p1_03` | | | | | | | | |
| `file7.jp2` | | | | | | | | |
| GRIB2-shaped | | | | | | | | |
| `regular_latlon` | | | | | | | | |
| tiled 2048, whole | | | | | | | | |

### Window and tile of the tiled file

Only decoders that can decode a region have these rows. OpenJPH reads HT
files only, and hayro-jpeg2000 has no region decode.

| Region | OpenJPEG 1 thread | OpenJPEG all cores | Grok 1 thread | Grok all cores | rust-j2k CLI | rust-j2k in process |
| ------ | ----------------- | ------------------ | ------------- | -------------- | ------------ | ------------------- |
| 256 x 256 window across four tiles | | | | | | |
| 64 x 64 window inside one tile | | | | | | |
| tile 27 | | | | | | |

### HTJ2K files

| File | OpenJPEG 1 thread | OpenJPEG all cores | Grok 1 thread | Grok all cores | OpenJPH | rust-j2k CLI | rust-j2k in process |
| ---- | ----------------- | ------------------ | ------------- | -------------- | ------- | ------------ | ------------------- |
| `ds0_ht_01_b11` | | | | | | | |
| `hifi_ht1_02` | | | | | | | |
| `ds1_ht_05_b11` | | | | | | | |
| `ds1_ht_04_b9` | | | | | | | |
| `ds0_hm_15_b8` (MIXED) | | | | | | | |
| `ds0_hm_06_b11` (MIXED) | | | | | | | |
| `ds1_ht_02_b11` | | | | | | | |
| `ds0_ht_05_b11` | | | | | | | |
| `ds0_ht_07_b16` | | | | | | | |
| generated 2048 x 2048 gray | | | | | | | |

hayro-jpeg2000 does not read HTJ2K: it stops on the first code-block style it
does not know, so it has no column here.

### Incremental decoder

`IncrementalDecoder` fed in 64 KB pieces, with `refine` called after each, and
`finish` at the end, against one `decode_with` of the same bytes. No other
decoder here has an incremental API, so there is nothing to compare it with.

| File | One decode, ms | 64 KB pieces, ms | Ratio |
| ---- | -------------- | ---------------- | ----- |
| GRIB2-shaped | | | |
| `ds0_ht_05_b11` | | | |

### Files more than 2x slower than OpenJPEG

TO FILL after the timing run: each file where rust-j2k's single-thread time is
more than twice OpenJPEG's, with an explanation or the issue that tracks it.

### Notes

TO FILL after the timing run.

## Output check

Every decoder's output was compared with the oracle before it was timed. A
decoder whose image was wrong has a note here and no time above.

The oracle is `opj_decompress` 2.5.0, writing PGX. OpenJPEG 2.5.0 does not
decode most of the HTJ2K conformance files: it refuses the MIXED code-block
set and fails on the others listed below. For those the oracle is the ISO/IEC 15444-4
reference image that `tests/fixtures/conformance/ht/` holds, and a lossy
result counts as right inside the bound that manifest gives. Lossless files
must match exactly. For a lossy file against OpenJPEG, a peak absolute error
(PAE) of 5 or less counts as right, the loosest bound the test suite uses for
a lossy file (16-bit).

`PAE n` is the largest difference of any sample, over all components.

| File | Oracle | OpenJPEG | Grok | OpenJPH | rust-j2k | hayro-jpeg2000 |
| ---- | ------ | -------- | ---- | ------- | -------- | -------------- |
| `p0_07` | OpenJPEG | exact | exact | n/a | exact | exact |
| `p0_08` | OpenJPEG | exact | exact | n/a | exact | exact |
| `p0_05` | OpenJPEG | exact | PAE 1 | n/a | PAE 1 | PAE 1, but see notes |
| `p1_03` | OpenJPEG | exact | PAE 1 | n/a | PAE 1 | PAE 1, but see notes |
| `file7.jp2` | OpenJPEG | exact | exact | n/a | exact | PAE 24574: no ICC conversion |
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

- Every decoder is a release build. Each cell is the median of RUNS runs, with
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
  A window or tile is a `DecodeOptions` window or tile.

## Machine

CPU: Intel Core i7-10700KF at 3.8 GHz, 8 cores and 16 threads. The CPU frequency governor was GOVERNOR (`run.py time` prints it);
it was not changed, and it affects absolute times. Load average
during the timing run: LOAD (the run waits for it to be under 2).

## Decoders and builds

| Decoder | Version | Source | Build |
| ------- | ------- | ------ | ----- |
| rust-j2k | RELEASE, commit COMMIT | this repository | `cargo build --release`, Rust RUSTC, default release profile |
| OpenJPEG | 2.5.0 (`opj_decompress`) | the Ubuntu 24.04 package, `2.5.0-2ubuntu0.5` | as the distribution built it |
| Grok | 20.4.25, commit `46ab34de` | tag `v20.4.25` of GrokImageCompression/grok, built from source | CMake, `Release` (`-O3 -DNDEBUG`), g++ 13.3, shared libraries, no sudo; LittleCMS bundled, the other libraries from the system |
| OpenJPH | 0.33.0, commit `345a8bcd` | tag `0.33.0` of aous72/OpenJPH, built from source | CMake, `Release` (`-O3 -DNDEBUG`), g++ 13.3 |
| hayro-jpeg2000 | 0.5.0 | crates.io | `cargo build --release`, features `std` and `simd`, same Rust as rust-j2k |

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
python3 run.py time --runs 5          # the speed and memory tables
python3 run.py incremental --runs 5   # the incremental table
```

`run.py time` and `run.py check` take `--only SUBSTRING` to run the files whose
name contains it. `perf-decode` also runs by hand, for example
`perf-decode rust-j2k FILE --iters 10` for in-process times, and
`perf-decode hayro FILE --out PREFIX` to write the components. Its header
comment lists every flag.
