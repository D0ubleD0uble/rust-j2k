//! HT code-blocks whose VLC stream has a set stuffing bit, against OpenJPEG.
//!
//! Each pair in `tests/fixtures/ht_vlc_stuffing/` is one cleanup-only HT
//! block twice: `.set.j2k` with one or two set VLC stuffing bits, and
//! `.clear.j2k` with those bits cleared. T.814 7.1.4 skips a stuffing bit, so
//! this decoder reads the two the same; OpenJPEG 2.5.0 ORs a set bit into the
//! next byte, so it decodes the `.set` file differently or refuses it (see
//! `docs/openjpeg-departures.md`, "Set stuffing bits in the HT VLC and MagRef
//! streams"). On the `abort_mel_` pairs the set bit follows a MEL 0xFF, and
//! OpenJPEG aborts on an assertion instead. On the cleared file the two
//! decoders agree. The pairs come from
//! a random differential of cleanup-only blocks, where every block the two
//! decoders read differently had a set VLC stuffing bit and none differed
//! once the bits were cleared. `manifest.tsv` holds the digests of OpenJPEG's
//! decodes, so the test needs no OpenJPEG at runtime.

mod support;

use std::path::PathBuf;

use support::jp2::sha256_hex;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ht_vlc_stuffing")
}

/// A pair's name, then OpenJPEG's digest of the `.set` file (`refused` when
/// it fails on it, `abort` when it aborts) and of the `.clear` file.
fn manifest() -> Vec<(String, String, String)> {
    let text = std::fs::read_to_string(dir().join("manifest.tsv")).unwrap();
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(cols.len(), 3, "manifest line {line:?}");
            (cols[0].to_owned(), cols[1].to_owned(), cols[2].to_owned())
        })
        .collect()
}

fn read(name: &str, tag: &str) -> Vec<u8> {
    let path = dir().join(format!("{name}.{tag}.j2k"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The decoded samples, each as a little-endian `i32`, as the manifest hashes
/// them.
fn samples(name: &str, tag: &str) -> Vec<u8> {
    let image = rust_j2k::decode(&read(name, tag)).unwrap_or_else(|e| panic!("{name}.{tag}: {e}"));
    assert_eq!(image.components.len(), 1);
    image.components[0]
        .samples
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect()
}

#[test]
fn a_cleared_block_decodes_as_openjpeg_decodes_it() {
    let rows = manifest();
    for (name, _, clear) in &rows {
        assert_eq!(sha256_hex(&samples(name, "clear")), *clear, "{name}");
    }
    assert_eq!(rows.len(), 16);
}

/// The set bits change nothing here, and OpenJPEG's decode of the `.set`
/// file differs from it.
#[test]
fn a_set_vlc_stuffing_bit_is_skipped_where_openjpeg_reads_it() {
    let mut refused = 0;
    for (name, set, _) in manifest() {
        let ours = samples(&name, "set");
        assert_eq!(ours, samples(&name, "clear"), "{name}");
        assert_ne!(sha256_hex(&ours), set, "{name}");
        refused += usize::from(set == "refused");
    }
    // OpenJPEG refuses three as malformed.
    assert_eq!(refused, 3);
}

/// Where OpenJPEG's `mel_init` assertion aborts on the `.set` file (a MEL
/// 0xFF followed by a byte above 0x8F, in a build with assertions), the block
/// decodes, as its cleared twin does. This pins only that: the MEL decode
/// stops before the 0xFF, and the VLC either never reads past it (2x17) or
/// reads next a byte whose low bit is already 1 (0xD9 on 4x16), so these
/// pairs cannot tell skipping the stuffing bit from ORing it in.
#[test]
fn a_set_stuffing_bit_after_a_mel_0xff_decodes_as_its_cleared_twin() {
    let aborts: Vec<String> = manifest()
        .into_iter()
        .filter(|(_, set, _)| set == "abort")
        .map(|(name, _, _)| name)
        .collect();
    assert_eq!(aborts, ["abort_mel_4x16_r12", "abort_mel_2x17_r8"]);
    for name in &aborts {
        let ours = samples(name, "set");
        assert!(ours.iter().any(|&b| b != 0), "{name}");
        assert_eq!(ours, samples(name, "clear"), "{name}");
    }
}

/// The two files of a pair differ only in the top bit of the bytes that
/// carry a VLC stuffing bit (7.1.4), each set in the `.set` file.
#[test]
fn a_pair_differs_only_in_its_vlc_stuffing_bits() {
    for (name, _, _) in manifest() {
        let set = read(&name, "set");
        let clear = read(&name, "clear");
        assert_eq!(set.len(), clear.len(), "{name}");
        // The cleanup segment ends just before EOC, and its last two bytes
        // hold `Scup`. The VLC stream starts in the last byte but one and
        // runs backward to `Pcup`.
        let end = set.len() - 2;
        assert_eq!(&set[end..], [0xFF, 0xD9], "{name}");
        let scup = 16 * usize::from(set[end - 1]) + usize::from(set[end - 2] & 0x0F);
        // The stuffing bytes as a decoder reads the `.set` file.
        let mut stuffing = Vec::new();
        if (set[end - 2] >> 4) & 7 == 7 {
            stuffing.push(end - 2);
        }
        let mut last = set[end - 2];
        for pos in (end - scup..end - 2).rev() {
            if last > 0x8F && set[pos] & 0x7F == 0x7F {
                stuffing.push(pos);
            }
            last = set[pos];
        }
        let differ: Vec<usize> = (0..set.len()).filter(|&i| set[i] != clear[i]).collect();
        assert!(!differ.is_empty(), "{name}");
        for &i in &differ {
            assert_eq!(set[i] ^ clear[i], 0x80, "{name}: byte {i}");
            assert_ne!(set[i] & 0x80, 0, "{name}: byte {i}");
            assert!(
                stuffing.contains(&i),
                "{name}: byte {i} is no stuffing byte"
            );
        }
    }
}

#[test]
fn the_manifest_lists_every_pair() {
    let mut files: Vec<String> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|f| f.ends_with(".j2k"))
        .collect();
    files.sort();
    let rows = manifest();
    let digest = |d: &str| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit());
    for (name, set, clear) in &rows {
        assert!(
            digest(set) || set == "refused" || set == "abort",
            "{name}: {set}"
        );
        assert!(digest(clear), "{name}: {clear}");
        assert_eq!(set == "abort", name.starts_with("abort_"), "{name}");
    }
    let mut want: Vec<String> = rows
        .iter()
        .flat_map(|(name, _, _)| [format!("{name}.clear.j2k"), format!("{name}.set.j2k")])
        .collect();
    want.sort();
    assert_eq!(files, want);
}
