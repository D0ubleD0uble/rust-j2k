//! PGX reader — the reference image format of the ISO/IEC 15444-4 corpus.
//!
//! One `.pgx` file holds a single component on its own sample grid. This reader
//! is shared by the Part 4 grading harness (`tests/conformance_part4.rs`); the
//! JP2 (Phase 3) and HTJ2K (Phase 4) conformance sets will reuse the same PGX
//! references, so the parser lives here rather than in any one test binary.

/// A decoded `.pgx` reference image: one component on its own sample grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pgx {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub signed: bool,
    pub samples: Vec<i32>,
}

/// Parse a `.pgx` reference image.
///
/// The header is one ASCII line — `PG <endian> <depth> <width> <height>` — then
/// the samples, packed at the natural byte width for the depth (1 byte up to 8
/// bits, 2 up to 16, 4 beyond) in the declared byte order.
///
/// Four details the corpus actually exercises and a naive reader gets wrong:
///
/// - the sign is part of the depth token, and may be glued (`-4`, `+8`) or
///   separated (`+ 8`); it may also be absent (`8`), meaning unsigned;
/// - fields are separated by *runs* of whitespace (`PG ML  8 17 37`), so the
///   header must be tokenized, not split on single spaces;
/// - three references end their header with CRLF, so the header must be
///   whitespace-trimmed or the `\r` contaminates the height token;
/// - signed samples are sign-extended from the **storage byte width**, not from
///   the declared depth: `p0_03` is 4-bit signed stored one sample per `i8`.
///
/// The byte width follows OpenJPEG's writer: 1 byte to 8 bits, 2 to 16, 4
/// beyond. (This is `ceil(depth / 8)` only up to 16 bits; 17..=24 uses 4, not 3.)
///
/// `endian` is `ML` (big) or `LM` (little). Every reference in the corpus is
/// `ML`; `LM` is accepted because the format allows it.
pub fn parse_pgx(bytes: &[u8]) -> Result<Pgx, String> {
    let newline = bytes
        .iter()
        .position(|&b| b == b'\n')
        .ok_or("no header line")?;
    let header = std::str::from_utf8(&bytes[..newline]).map_err(|_| "header is not UTF-8")?;

    let mut tokens = header.split_ascii_whitespace();
    match tokens.next() {
        Some("PG") => {}
        other => return Err(format!("bad magic: expected `PG`, got {other:?}")),
    }
    let big_endian = match tokens.next() {
        Some("ML") => true,
        Some("LM") => false,
        other => return Err(format!("bad byte order: expected `ML`/`LM`, got {other:?}")),
    };

    // The depth token, re-joined if the sign was written as its own token.
    let mut depth_token = tokens.next().ok_or("missing depth")?.to_string();
    if depth_token == "+" || depth_token == "-" {
        depth_token.push_str(tokens.next().ok_or("missing depth after sign")?);
    }
    let depth: i32 = depth_token
        .strip_prefix('+')
        .unwrap_or(&depth_token)
        .parse()
        .map_err(|_| format!("bad depth `{depth_token}`"))?;
    let signed = depth < 0;
    let bit_depth = depth.unsigned_abs();
    if !(1..=32).contains(&bit_depth) {
        return Err(format!("depth {bit_depth} outside 1..=32"));
    }

    let mut dimension = |what: &str| -> Result<u32, String> {
        tokens
            .next()
            .ok_or_else(|| format!("missing {what}"))?
            .parse()
            .map_err(|_| format!("bad {what}"))
    };
    let width = dimension("width")?;
    let height = dimension("height")?;

    let stride = match bit_depth {
        1..=8 => 1usize,
        9..=16 => 2,
        _ => 4,
    };
    let count = (width as usize)
        .checked_mul(height as usize)
        .ok_or("width * height overflows")?;
    let needed = count.checked_mul(stride).ok_or("body size overflows")?;

    // Exact, not `<`: every reference in the corpus is exactly its body, so a
    // short file is truncation and a long one is a header that disagrees with
    // its payload. Both should fail loudly rather than grade a prefix.
    let body = &bytes[newline + 1..];
    if body.len() != needed {
        return Err(format!(
            "body is {} bytes but {width}x{height} at {stride} byte(s)/sample needs {needed}",
            body.len()
        ));
    }

    let samples = body
        .chunks_exact(stride)
        .map(|chunk| {
            // Widen to `u32` most-significant byte first, then reinterpret at the
            // storage width so a signed sample sign-extends correctly.
            let fold = |acc: u32, &b: &u8| (acc << 8) | b as u32;
            let raw = if big_endian {
                chunk.iter().fold(0u32, fold)
            } else {
                chunk.iter().rev().fold(0u32, fold)
            };
            match (signed, stride) {
                // Samples land in `i32`, matching `rust_j2k::Component`. An
                // unsigned value past `i32::MAX` has no representation there, so
                // reject it rather than wrap it to a negative reference sample.
                (false, _) if raw > i32::MAX as u32 => {
                    Err(format!("unsigned sample {raw} exceeds the i32 container"))
                }
                (false, _) => Ok(raw as i32),
                (true, 1) => Ok(raw as u8 as i8 as i32),
                (true, 2) => Ok(raw as u16 as i16 as i32),
                (true, _) => Ok(raw as i32),
            }
        })
        .collect::<Result<Vec<i32>, String>>()?;

    Ok(Pgx {
        width,
        height,
        bit_depth: bit_depth as u8,
        signed,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pgx(header: &str, body: &[u8]) -> Vec<u8> {
        let mut bytes = header.as_bytes().to_vec();
        bytes.push(b'\n');
        bytes.extend_from_slice(body);
        bytes
    }

    #[test]
    fn pgx_reads_unsigned_bytes() {
        let image = parse_pgx(&pgx("PG ML 8 2 2", &[0, 127, 128, 255])).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!((image.bit_depth, image.signed), (8, false));
        assert_eq!(image.samples, vec![0, 127, 128, 255]);
    }

    /// The sign may be glued to the depth (`+8`), stand alone (`+ 8`), or be
    /// absent — and fields may be separated by runs of whitespace. All four
    /// spellings appear in the corpus.
    #[test]
    fn pgx_accepts_every_depth_spelling() {
        let body = [1u8, 2];
        for header in [
            "PG ML +8 2 1",
            "PG ML + 8 2 1",
            "PG ML  8 2 1",
            "PG ML 8 2 1",
        ] {
            let image = parse_pgx(&pgx(header, &body)).unwrap_or_else(|e| panic!("{header}: {e}"));
            assert!(!image.signed, "{header}: should be unsigned");
            assert_eq!(image.bit_depth, 8, "{header}");
            assert_eq!(image.samples, vec![1, 2], "{header}");
        }
    }

    /// A negative depth means signed, and samples sign-extend from the storage
    /// byte width — not from the declared depth. `p0_03` is 4-bit signed stored
    /// one sample per byte, with values down to -8 (`0xF8`).
    #[test]
    fn pgx_sign_extends_from_the_storage_width() {
        let image = parse_pgx(&pgx("PG ML -4 3 1", &[0xF8, 0x00, 0x05])).unwrap();
        assert!(image.signed);
        assert_eq!(image.bit_depth, 4);
        assert_eq!(image.samples, vec![-8, 0, 5]);

        // 12-bit signed occupies two bytes; -1 is 0xFFFF, not 0x0FFF.
        let image = parse_pgx(&pgx("PG ML -12 2 1", &[0xFF, 0xFF, 0x07, 0xFF])).unwrap();
        assert_eq!(image.samples, vec![-1, 2047]);
    }

    /// Samples land in `i32`. An unsigned 32-bit sample past `i32::MAX` has no
    /// representation there and must be rejected, not wrapped to a negative
    /// reference sample that would then be graded against.
    #[test]
    fn pgx_rejects_an_unsigned_sample_past_the_i32_container() {
        let err = parse_pgx(&pgx("PG ML 32 1 1", &[0xFF, 0xFF, 0xFF, 0xFF])).unwrap_err();
        assert!(err.contains("exceeds the i32 container"), "got {err}");

        // The largest representable unsigned sample still reads back exactly.
        let image = parse_pgx(&pgx("PG ML 32 1 1", &[0x7F, 0xFF, 0xFF, 0xFF])).unwrap();
        assert_eq!(image.samples, vec![i32::MAX]);

        // Signed 32-bit spans the whole container, so nothing is rejected.
        let image = parse_pgx(&pgx("PG ML -32 1 1", &[0xFF, 0xFF, 0xFF, 0xFF])).unwrap();
        assert_eq!(image.samples, vec![-1]);
    }

    /// Three corpus references (`c0p0_03r0`, `c0p0_03r1`, `c0p0_14`) terminate
    /// their header with CRLF. The `\r` must not leak into the height token, and
    /// the body must still start after the `\n`.
    #[test]
    fn pgx_accepts_a_crlf_header() {
        let mut bytes = b"PG ML -4 2 1\r\n".to_vec();
        bytes.extend_from_slice(&[0xF8, 0x05]);
        let image = parse_pgx(&bytes).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.samples, vec![-8, 5]);
    }

    /// Depths above 8 bits pack big-endian by default (`ML`); `LM` is little-endian.
    #[test]
    fn pgx_honours_byte_order() {
        let big = parse_pgx(&pgx("PG ML 12 2 1", &[0x0F, 0xFF, 0x00, 0x01])).unwrap();
        assert_eq!(big.samples, vec![4095, 1]);

        let little = parse_pgx(&pgx("PG LM 12 2 1", &[0xFF, 0x0F, 0x01, 0x00])).unwrap();
        assert_eq!(little.samples, vec![4095, 1]);
    }

    #[test]
    fn pgx_rejects_malformed_input() {
        // A truncated body is the failure the corpus most needs caught.
        let err = parse_pgx(&pgx("PG ML 8 4 4", &[0; 15])).unwrap_err();
        assert!(err.contains("needs 16"), "got {err}");

        for (bytes, what) in [
            (pgx("XX ML 8 1 1", &[0]), "magic"),
            (pgx("PG XX 8 1 1", &[0]), "byte order"),
            (pgx("PG ML 8 1", &[0]), "missing height"),
            (pgx("PG ML 0 1 1", &[0]), "depth 0"),
            (pgx("PG ML 33 1 1", &[0]), "depth 33"),
            (b"PG ML 8 1 1".to_vec(), "no newline"),
        ] {
            assert!(parse_pgx(&bytes).is_err(), "{what} should be rejected");
        }
    }
}
