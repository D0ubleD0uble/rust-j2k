# rust-j2k

[![CI](https://github.com/D0ubleD0uble/rust-j2k/actions/workflows/ci.yml/badge.svg)](https://github.com/D0ubleD0uble/rust-j2k/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/rust-j2k.svg)](https://crates.io/crates/rust-j2k)
[![docs.rs](https://img.shields.io/docsrs/rust-j2k)](https://docs.rs/rust-j2k)

rust-j2k decodes JPEG 2000 images in pure Rust, for programs that need to read
them without linking a C library. It reads `.jp2` and `.jph` files, bare
`.j2k` and `.j2c` streams, and the JPEG 2000 grids inside GRIB2 weather files,
and gives you each channel's values as whole numbers at the file's full depth.
It has no dependencies, so it cross-compiles without a C toolchain. Its output
is checked against [OpenJPEG](https://www.openjpeg.org/), an official JPEG 2000
reference implementation, and against the standard's own conformance tests.

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
values come back as stored and `jp2.color` is still
`ColorSpace::RestrictedIcc`.
`decode_with` takes the other options too,
such as a smaller image. To learn the size and channels without decoding, call
`read_header`. The [API docs](https://docs.rs/rust-j2k) cover each one.

A bare `.j2k` or `.j2c` stream has no boxes, so `image.jp2` is `None` and the
file says nothing about its color space. One channel is usually gray, and two
are gray plus transparency; for three or more, you have to know what they
mean.

### Decode part of an image

`read_header` reads the size, the image area and the tile grid without
decoding. Pass a rectangle or a tile number to `decode_with` to decode only
that part. Tiles and the reference grid are explained in [What a JPEG 2000
file holds](#what-a-jpeg-2000-file-holds).

```rust
use rust_j2k::{DecodeOptions, Region, Window, decode_with, read_header};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read("YOUR_FILE.jp2")?;
    let header = read_header(&bytes)?;
    println!("{}x{}, {} tiles", header.width, header.height, header.tiles.count());

    // The image's top-left 256 × 256. The image area can start away from
    // the grid's origin, so place the window from it.
    let area = header.image_area;
    let (x1, y1) = (area.x0.saturating_add(256), area.y0.saturating_add(256));
    let window = Region::Window(Window::new(area.x0, area.y0, x1, y1));
    let part = decode_with(&bytes, &DecodeOptions::default().with_region(window))?;
    println!("window: {}x{}", part.width, part.height);

    // The last tile. Tiles are numbered row by row from 0, and
    // `header.tiles.tile(n)` gives tile n's rectangle.
    let last = Region::Tile(header.tiles.count() - 1);
    let tile = decode_with(&bytes, &DecodeOptions::default().with_region(last))?;
    println!("last tile: {}x{}", tile.width, tile.height);
    Ok(())
}
```

The rectangle is on the reference grid. It must overlap the image area, or
`decode_with` returns `Error::InvalidOptions`; the part of it outside the
image area is cut off.

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

The image sits on a **reference grid**, the coordinate space of the whole
codestream. The image area can start away from the grid's origin, and each
channel's grid is the image area divided by its sub-sampling. The image is
cut into rectangular **tiles**, numbered row by row from 0, each coded on its
own. A file with no tiling is one tile.

The API docs use the standard's names for the parts of a codestream:

- A **marker** is a two-byte code that names the header segment after it,
  or marks a boundary. The main header, before any tile, holds `SIZ` (the
  image size, tile size and channels) and `COD` (how the image is coded),
  among others.
- Each tile is stored as one or more **tile-parts**. Each tile-part starts
  with an `SOT` marker, which gives its tile and its length (`Psot`; 0 means
  "to the end of the codestream"), and can give the tile's number of
  tile-parts (`TNsot`; 0 means not declared). The first `SOT` marker ends
  the main header.
- A tile's coded data is a series of **packets**. Each packet header says
  which **code-blocks**, small blocks of the image's wavelet coefficients,
  it adds data to. Reading those headers is the **packet walk**.
- The `EOC` marker ends the codestream.

## What it decodes

- It reads **every coding feature of the core standard** (ISO/IEC 15444-1,
  "Part 1"): lossless and lossy compression, images cut into tiles, quality
  layers, every order the data can be stored in, and regions coded at higher
  quality.
  [docs/scope.md](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/docs/scope.md) lists every feature.
- It reads **HTJ2K** (ISO/IEC 15444-15, "Part 15"), the high-throughput
  variant, including files that mix HT and Part 1 code-blocks, and `.jph`
  files, the HTJ2K version of `.jp2`. You don't need to know whether a file
  uses it: the same call and options decode either kind.
- It reads **the `.jp2` file format**: palettes, channel labels, the color
  space, the resolution, and the XML and UUID metadata boxes. It applies the
  palette and puts the color channels in order, and reports the rest on
  `Image::jp2`.
- It can **convert to sRGB** if you ask (`ColorOutput::Srgb`). It converts
  sYCC, and the restricted ICC profiles JP2 allows (gray, or RGB defined by a
  matrix and curves), giving the same samples as OpenJPEG with LittleCMS.
- It can **decode a smaller image**. Each resolution level you drop about halves the
  width and height (`DecodeOptions::with_resolution_reduction`).
- It can **decode fewer quality layers**, for a coarser image at full size in
  less time (`DecodeOptions::with_max_quality_layers`).
- It can **decode part of an image**: a rectangle of it, one tile, or some of
  its components (`DecodeOptions::with_region`,
  `with_components`). Only the tiles the rectangle reaches are decoded, and in
  them only the code-blocks it needs.
- It can **decode a codestream that is cut short** and tell you what is
  missing (`DecodeOptions::with_strict(false)`), and decode a file while it
  arrives (`IncrementalDecoder`).
- It reads **whole numbers up to 32 bits**: unsigned up to 31 bits and signed
  up to 32, with or without sub-sampling.

## What it doesn't do

- It doesn't **encode**. It reads JPEG 2000 but can't write it. An encoder is
  planned ([docs/roadmap.md](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/docs/roadmap.md)). To write JPEG 2000 today, use
  OpenJPEG's `opj_compress`.
- It doesn't **give you display pixels**. You get each channel as its own
  `Vec<i32>` at the file's depth and in the file's color space, not 8-bit RGBA.
  To show an image, ask for sRGB and scale each value to 8 bits yourself (see
  [Decode a file](#decode-a-file)).
- It doesn't **read the Part 2 extensions**, or an HTJ2K codestream that
  needs more than one HT coding pass set per code-block. These are refused with
  `Error::Unsupported`.
- It doesn't **open JPX, JPM or MJ2 files** unless they say they can also be
  read as JP2 or JPH. Those files are refused with `Error::Unsupported`.
- It doesn't **read unsigned 32-bit channels** or channels deeper than 32 bits,
  because they don't fit an `i32`. The standard allows up to 38. These are
  refused with `Error::Unsupported`.
- It doesn't **convert every color space to sRGB**. CMYK, CIELab, e-sYCC, and
  ICC profiles that work through a lookup table or through Lab, are refused with
  `Error::Unsupported` when you ask for sRGB. Decoding without the option still
  gives you the values as stored.
  [docs/srgb-output.md](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/docs/srgb-output.md#every-input-and-its-result) lists
  every case.
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

Every failure comes back as an `Error`. With the default limits, decoding never
panics or hangs, whatever the input, and needs at most about 6.4 GiB of memory
for an input of up to 64 MiB, or about 102 bytes per byte of input past that. If
the machine can't supply that, the allocation fails and the process aborts, as
in any Rust program. Those figures cover the one-shot decode functions;
`IncrementalDecoder` keeps per-tile state and can hold up to about three times
one decode's peak, plus the bytes fed and a table of 24 bytes per tile index up
to the highest seen, about 1.5 MiB for 65535 tiles (see
[`docs/incremental-decoding.md`](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/incremental-decoding.md)).

`Error::Unsupported` usually means the file is valid JPEG 2000 that uses
something from [What it doesn't do](#what-it-doesnt-do). `Error::Limit` means
the file asks for more memory or work than the limits allow (see [Large
files](#large-files)). A damaged header can show up as either one.
`Error::InvalidOptions` means your options are invalid, or don't fit this
file (such as a reduction past its levels).
`Error::Inconsistent` is a bug in this decoder; please report it. The other
variants mean the input is damaged or isn't JPEG 2000.

```rust
use rust_j2k::{Error, decode};

fn check(bytes: &[u8]) {
    match decode(bytes) {
        Ok(image) => println!("decoded {} channels", image.components.len()),
        Err(Error::Unsupported(what)) => eprintln!("not decoded: {what}"),
        Err(Error::Limit(limit)) => eprintln!("past a limit: {limit}"),
        Err(e) => eprintln!("damaged or not JPEG 2000: {e}"),
    }
}
```

### Damaged files

By default a damaged or cut-off file is an error, never a partial image. What
is refused, and where this decoder is stricter than OpenJPEG, is in
[docs/openjpeg-departures.md](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/docs/openjpeg-departures.md#stricter-than-the-oracle-on-non-conforming-streams).

To decode what a cut-short codestream holds, as `opj_decompress -allow-partial`
does, turn `strict` off. The codestream's header, everything before the first
tile, must have arrived. Only missing data and extra bytes after a tile's last
packet are let through; any other damage is still an error. `Image::report`
says what was missing, and its `complete_reduction` is the finest resolution
the data already gives in full.

For a file that arrives in pieces, such as a download, use
`IncrementalDecoder`. Feed it each piece and call `refine`. From time to time
it returns an image of what has come in so far (`Refined::Image`); otherwise
it says why it waits (`Refined::Waiting`), such as for more bytes or for the
rest of the main header. Each refine goes on from where
the last one stopped reading packets;
[docs/incremental-decoding.md](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/incremental-decoding.md)
lists the packets it reads again. Each image `refine` returns is decoded with
`strict` off and comes with a report of what is missing. `finish` decodes
under the options you gave the decoder, so it is strict unless you turned
`strict` off:

```rust
use rust_j2k::{DecodeOptions, IncrementalDecoder, Refined};

// `pieces`: the parts of the file, in the order they arrive.
fn show_as_it_arrives(pieces: &[&[u8]]) -> Result<(), rust_j2k::Error> {
    let mut decoder = IncrementalDecoder::new(DecodeOptions::default())?;
    for piece in pieces {
        decoder.feed(piece);
        if let Refined::Image(image) = decoder.refine()? {
            // Show `image`. Its report says what is still missing.
            for entry in &image.report.entries {
                eprintln!("{entry}");
            }
        }
    }
    // The input has ended: decode all of it.
    let image = decoder.finish()?;
    println!("{}x{}", image.width, image.height);
    Ok(())
}
```

### Large files

A valid file can be refused with `Error::Limit` when it is very large or very
costly to decode. The limits protect you from a small hostile file that asks
for gigabytes of memory or minutes of work. By default an image may hold 2^29
values (2 GiB as `i32`): a 10980 × 10980 three-band satellite image, or an RGB
image of about 179 megapixels. Past 64 MiB of input the limit is 8 values per
byte of the file.

Three limits can be raised: `sample_limit`, `packet_work_limit` and
`code_block_work_limit` on `DecodeOptions`. For a file you trust, set them to
`WorkLimit::Unlimited`. The process then aborts if memory runs out, instead of
returning an error. Damage can also look like a limit, since one flipped bit
in the header can declare an absurd size, so don't lift the limits on that
evidence alone. The error says which limit refused the file and what to
raise it to, as data: `LimitDetail::kind` and `LimitDetail::retry_with`.
`DecodeOptions::with_retry` sets that limit for you:

```rust
use rust_j2k::{DecodeOptions, Error, Image, decode_with};

fn decode_raising_limits(bytes: &[u8]) -> Result<Image, Error> {
    let mut options = DecodeOptions::default();
    // A few retries; each one can ask for more.
    for _ in 0..4 {
        match decode_with(bytes, &options) {
            Err(Error::Limit(limit)) => match options.with_retry(&limit) {
                Some(raised) => options = raised,
                // No option raises this limit, or it gave no factor.
                None => return Err(Error::Limit(limit)),
            },
            result => return result,
        }
    }
    decode_with(bytes, &options)
}
```

A retry can be refused again, by the same limit or another, so the loop stops
after a few. The [`WorkLimit`
docs](https://docs.rs/rust-j2k/latest/rust_j2k/enum.WorkLimit.html) say how
far to trust the factor.

## How it's checked

It decodes all 23 test codestreams of the JPEG 2000 conformance suite
(ISO/IEC 15444-4) within the error the standard allows, the suite's nine JP2
files within theirs, and its 41 HTJ2K codestreams and 20 JPH files within
theirs. Lossless images, including GRIB2 grids written by eccodes, match
OpenJPEG bit for bit. The details are in
[docs/correctness.md](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/docs/correctness.md), and the places where this decoder
and OpenJPEG differ on purpose are in
[docs/openjpeg-departures.md](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/docs/openjpeg-departures.md). The test suite
needs no external tools.

The speed and memory of this decoder next to OpenJPEG, Grok, OpenJPH and
hayro-jpeg2000 are in [docs/performance.md](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/docs/performance.md).

## Contributing and security

To contribute, start with [CONTRIBUTING.md](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/CONTRIBUTING.md). It has the
workflow and the checks every pull request must pass. To report a crash, a hang
or another vulnerability, see [SECURITY.md](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/SECURITY.md).

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/LICENSE-APACHE))
- MIT license ([LICENSE-MIT](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/LICENSE-MIT))

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
