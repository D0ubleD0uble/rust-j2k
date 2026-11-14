# rust-j2k

[![CI](https://github.com/D0ubleD0uble/rust-j2k/actions/workflows/ci.yml/badge.svg)](https://github.com/D0ubleD0uble/rust-j2k/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/rust-j2k.svg)](https://crates.io/crates/rust-j2k)
[![docs.rs](https://img.shields.io/docsrs/rust-j2k)](https://docs.rs/rust-j2k)

rust-j2k decodes JPEG 2000 images in pure Rust, for programs that need to read
them without linking a C library. It reads `.jp2` files, bare `.j2k` and
`.j2c` streams, and the JPEG 2000 grids inside GRIB2 weather files, and gives
you each channel's values as whole numbers at the file's full depth. It has no
dependencies, so it cross-compiles without a C toolchain. Its output is checked
against [OpenJPEG](https://www.openjpeg.org/), an official JPEG 2000 reference
implementation, and against the standard's own conformance tests.

## Install

```sh
cargo add rust-j2k
```

It needs Rust 1.87 or newer.

## Decode a file

Pass the bytes to `decode` and you get back an `Image`, with one `Component`
for each channel. Start here:

```rust
use rust_j2k::decode;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A .jp2, .j2k or .j2c file, or the JPEG 2000 bytes of a GRIB2 grid.
    let bytes = std::fs::read("YOUR_FILE.jp2")?;
    let image = decode(&bytes)?;
    println!("{}x{}, {} channels", image.width, image.height, image.components.len());

    let channel = image.component(0).expect("an image has at least one channel");
    println!(
        "channel 0 is {}x{}, {}-bit, signed: {}",
        channel.width, channel.height, channel.bit_depth, channel.signed,
    );

    // `samples` holds width × height numbers, row by row.
    let first = channel.sample(0, 0).expect("0,0 is inside the grid");
    println!("first value: {first}");
    Ok(())
}
```

The values are the image's samples. Unsigned ones run from 0 to
2^bit_depth − 1, and signed ones are centered on 0.

To show a `.jp2` image, decode it with
`DecodeOptions::default().with_color(ColorOutput::Srgb)` through `decode_with`,
then scale each value to 8 bits; the [`ColorOutput::Srgb`
docs](https://docs.rs/rust-j2k/latest/rust_j2k/enum.ColorOutput.html#variant.Srgb)
show how. Check `image.jp2` afterwards. If a profile couldn't be applied, the
values come back as stored and its `color` still names the ICC profile.
`decode_with` takes the other options too,
such as a smaller image. To learn the size and channels without decoding, call
`read_header`. The [API docs](https://docs.rs/rust-j2k) cover each one.

## What a JPEG 2000 file holds

A JPEG 2000 image is stored as one compressed stream of bytes, called a
codestream. A `.j2k` or `.j2c` file is just that stream, and so is the JPEG
2000 part of a GRIB2 grid. A `.jp2` file wraps the stream in labeled sections,
called boxes. The boxes say what the colors mean, and they can carry a palette,
the capture and display resolution, or XML metadata.

The image has one or more channels: one for a grayscale picture or a weather
grid, three for RGB, and sometimes one more for transparency. Each channel is
its own grid of whole numbers, often 8 bits deep and sometimes 12 or 16. A
channel can be stored at a lower resolution than the image, for example a
color channel at half the width. This is called sub-sampling. That channel's
grid is smaller than the image, and its `x_sampling` and `y_sampling` say by
how much.

## What it decodes

- It reads **every coding feature of the core standard** (ISO/IEC 15444-1,
  "Part 1"): lossless and lossy compression, images cut into tiles, quality
  layers, every order the data can be stored in, and regions coded at higher
  quality.
  [docs/scope.md](docs/scope.md) lists every feature.
- It reads **the `.jp2` file format**: palettes, channel labels, the color
  space, the resolution, and the XML and UUID metadata boxes. It applies the
  palette and puts the color channels in order, and reports the rest on
  `Image::jp2`.
- It can **convert to sRGB** if you ask (`ColorOutput::Srgb`). It converts
  sYCC, and the restricted ICC profiles JP2 allows (gray, or RGB defined by a
  matrix and curves), giving the same samples as OpenJPEG with LittleCMS.
- It can **decode a smaller image**. Each resolution level you drop about halves the
  width and height (`DecodeOptions::with_resolution_reduction`).
- It can **decode a codestream that is cut short**, such as one still
  arriving, and tell you what is missing (`DecodeOptions::with_strict(false)`).
- It reads **whole numbers up to 32 bits**: unsigned up to 31 bits and signed
  up to 32, with or without sub-sampling.

## What it doesn't do

- It doesn't **encode**. It reads JPEG 2000 but can't write it. An encoder is
  planned ([docs/roadmap.md](docs/roadmap.md)). To write JPEG 2000 today, use
  OpenJPEG's `opj_compress`.
- It doesn't **give you display pixels**. You get each channel as its own
  `Vec<i32>` at the file's depth and in the file's color space, not 8-bit RGBA.
  To show an image, ask for sRGB and scale each value to 8 bits yourself (see
  [Decode a file](#decode-a-file)).
- It doesn't **read HTJ2K**, the high-throughput variant (Part 15), or the
  Part 2 extensions. It refuses them with `Error::Unsupported`. OpenJPEG 2.5
  and later decode HTJ2K.
- It doesn't **open JPX, JPM or MJ2 files** unless they say they can also be
  read as JP2. Those files are refused with `Error::Unsupported`.
- It doesn't **read unsigned 32-bit channels** or channels deeper than 32 bits,
  because they don't fit an `i32`. The standard allows up to 38. These are
  refused with `Error::Unsupported`.
- It doesn't **convert every color space to sRGB**. CMYK, CIELab, e-sYCC, and
  ICC profiles that work through a lookup table or through Lab, are refused with
  `Error::Unsupported` when you ask for sRGB. Decoding without the option still
  gives you the values as stored.
  [docs/srgb-output.md](docs/srgb-output.md#every-input-and-its-result) lists
  every case.
- It doesn't **decode part of an image**. It always decodes the whole image
  and every quality layer. For a smaller image, drop resolution levels
  instead. OpenJPEG's `opj_decompress` can decode a region (`-d`) or fewer
  layers (`-l`).
- It doesn't **read GRIB2 files**. You find the JPEG 2000 bytes in the message
  and turn the decoded numbers into physical values yourself (see
  [GRIB2 grids](#grib2-grids)).
- It can't **always detect damage**. JPEG 2000 has no checksum, so a flipped
  byte in the compressed data can decode to a wrong image with no error. If
  that matters, check the file against a hash you keep.

## GRIB2 grids

A GRIB2 message that uses data representation template 5.40 stores its grid as
a JPEG 2000 codestream in section 7, the data section. Pass `decode` the bytes
of section 7 after its 5-byte header. You get one channel of whole numbers X,
in the grid's scanning order. To get the physical values Y, take the reference
value R, the binary scale factor E and the decimal scale factor D from section
5, and compute Y = (R + X × 2^E) / 10^D (WMO regulation 92.9.4, note 4). If the
message has a bitmap in section 6, the values fill only the points it marks.

If section 5 gives 0 bits per value (octet 20), the field is constant and
section 7 may hold no codestream at all. Don't call `decode` then: every point
has the value R, which is how eccodes and NCEP's g2clib read it.

## When a file won't decode

Every failure comes back as an `Error`. With the default limits, decoding
never panics or hangs, whatever the input, and needs at most about 3.2 GiB of
memory. If the machine can't supply that, the allocation fails and the process
aborts, as in any Rust program.

`Error::Unsupported` usually means the file is valid JPEG 2000 that uses
something from [What it doesn't do](#what-it-doesnt-do). `Error::Limit` means
the file asks for more memory or work than the limits allow (see [Large
files](#large-files)). A damaged header can show up as either one.
`Error::InvalidOptions` means your options don't fit the file.
`Error::Inconsistent` is a bug in this decoder; please report it. The other
variants mean the input is damaged or isn't JPEG 2000.

```rust
use rust_j2k::{Error, decode};

fn check(bytes: &[u8]) {
    match decode(bytes) {
        Ok(image) => println!("decoded {} channels", image.components.len()),
        Err(Error::Unsupported(what)) => eprintln!("not decoded: {what}"),
        Err(Error::Limit(what)) => eprintln!("past a limit: {what}"),
        Err(e) => eprintln!("damaged or not JPEG 2000: {e}"),
    }
}
```

### Damaged files

By default a damaged or cut-off file is an error, never a partial image. What
is refused, and where this decoder is stricter than OpenJPEG, is in
[docs/openjpeg-departures.md](docs/openjpeg-departures.md#stricter-than-the-oracle-on-non-conforming-streams).

To decode what a cut-short codestream holds, as `opj_decompress -allow-partial`
does, turn `strict` off. The codestream's header, everything before the first
tile, must have arrived. Only missing data and extra bytes after a tile's last
packet are let through; any other damage is still an error. `Image::report`
says what was missing, and its `complete_reduction` is the finest resolution
the data already gives in full, so you can decode a codestream while it
arrives:

```rust
use rust_j2k::{DecodeOptions, decode_with};

// `prefix`: the bytes of the codestream that have arrived so far.
fn show_missing(prefix: &[u8]) -> Result<(), rust_j2k::Error> {
    let options = DecodeOptions::default().with_strict(false);
    let image = decode_with(prefix, &options)?;
    if !image.report.is_complete() {
        for entry in &image.report.entries {
            eprintln!("{entry}");
        }
    }
    Ok(())
}
```

### Large files

A valid file can be refused with `Error::Limit` when it is very large or very
costly to decode. The limits protect you from a small hostile file that asks
for gigabytes of memory or minutes of work. By default an image may hold 2^28
values (1 GiB as `i32`): a 10980 × 10980 satellite band, or an RGB image of
about 89 megapixels.

Three limits can be raised: `sample_limit`, `packet_work_limit` and
`code_block_work_limit` on `DecodeOptions`. For a file you trust, set them to
`WorkLimit::Unlimited`. The process then aborts if memory runs out, instead of
returning an error. Damage can also look like a limit, since one flipped bit
in the header can declare an absurd size, so don't lift the limits on that
evidence alone. The [`WorkLimit`
docs](https://docs.rs/rust-j2k/latest/rust_j2k/enum.WorkLimit.html) show a
retry.

## How it's checked

It decodes all 23 test codestreams of the JPEG 2000 conformance suite
(ISO/IEC 15444-4) within the error the standard allows, and the suite's nine
JP2 files within theirs. Lossless images, including GRIB2 grids written by
eccodes, match OpenJPEG bit for bit. The details are in
[docs/correctness.md](docs/correctness.md), and the places where this decoder
and OpenJPEG differ on purpose are in
[docs/openjpeg-departures.md](docs/openjpeg-departures.md). The test suite
needs no external tools.

## Contributing and security

To contribute, start with [CONTRIBUTING.md](CONTRIBUTING.md). It has the
workflow and the checks every pull request must pass. To report a crash, a hang
or another vulnerability, see [SECURITY.md](SECURITY.md).

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

### Third-party code

`src/jp2/lcms.rs` is a Rust port of parts of LittleCMS 2.14, which is MIT
licensed. Its copyright and license notice is at the top of that file.

`src/jp2/math.rs` and `src/jp2/math_data.rs` are a Rust port of `pow` and
`log` from Arm's optimized-routines (commit `2ee4b0d`), used under the MIT
license. Its copyright and license notice is at the top of `math.rs`.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual-licensed as above, without any additional terms or conditions.
