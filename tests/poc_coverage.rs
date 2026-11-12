//! A `POC` marker whose volumes leave out packets of the tile: the packets
//! they do not reach are missing from the codestream, and the decode refuses
//! it with [`Error::Codestream`] rather than decode them as empty.
//! `opj_compress` 2.5.0 writes only the packets its volumes reach, and
//! `opj_decompress` 2.5.0 decodes the rest as empty, so its image is not the
//! source. The files are written by `scripts/gen-poc-coverage-fixtures.py`;
//! see `tests/fixtures/poc_coverage/README.md`.

use std::path::PathBuf;

use rust_j2k::{Error, decode, read_header};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/poc_coverage")
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The source's samples, big-endian 16-bit, component after component.
fn source(name: &str) -> Vec<i32> {
    read(name)
        .chunks_exact(2)
        .map(|b| i32::from(u16::from_be_bytes([b[0], b[1]])))
        .collect()
}

fn assert_refused(name: &str, bytes: &[u8], component: usize, resolution: usize) {
    match decode(bytes) {
        Err(Error::Codestream(m)) => assert!(
            m.contains("leave out")
                && m.contains(&format!("component {component}, resolution {resolution},")),
            "{name}: {m}"
        ),
        Err(e) => panic!("{name}: expected a codestream error, got {e}"),
        Ok(_) => panic!("{name}: expected a codestream error, got an image"),
    }
}

fn assert_source(name: &str, bytes: &[u8], source_name: &str) {
    let image = decode(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    let samples: Vec<i32> = image
        .components
        .iter()
        .flat_map(|c| c.samples.iter().copied())
        .collect();
    assert!(samples == source(source_name), "{name}: not the source");
}

/// `-n 2 -POC T1=0,0,1,1,1,LRCP`: one volume over resolution 0 of two. The
/// packet of resolution 1 is missing; `opj_decompress` decodes every sample
/// wrong, by up to 993.
#[test]
fn a_resolution_left_out_is_refused() {
    let name = "poc_res_missing.j2k";
    let bytes = read(name);
    assert_refused(name, &bytes, 0, 1);
    // The header does not walk a raw codestream's packets.
    read_header(&bytes).unwrap();
}

/// `-POC T1=0,3,20,3,4,CPRL` over four components: one volume over the
/// last. Components 0 to 2 are missing; `opj_decompress` decodes them as 0
/// and 2048.
#[test]
fn components_left_out_are_refused() {
    let name = "poc_comp_missing.j2k";
    let bytes = read(name);
    assert_refused(name, &bytes, 0, 0);
    read_header(&bytes).unwrap();
}

/// The same encodes with a second volume over the rest decode to the
/// source, as `opj_decompress` decodes them.
#[test]
fn volumes_that_cover_the_tile_decode() {
    assert_source(
        "poc_res_covered.j2k",
        &read("poc_res_covered.j2k"),
        "res_source.raw",
    );
    assert_source(
        "poc_comp_covered.j2k",
        &read("poc_comp_covered.j2k"),
        "comp_source.raw",
    );
}

/// The volumes of every tile-part count: here the second volume, over
/// resolution 1, is in the second tile-part's header.
#[test]
fn a_later_tile_parts_volume_covers_the_rest() {
    let name = "poc_res_tileparts.j2k";
    assert_source(name, &read(name), "res_source.raw");
}

fn jp2_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(8 + payload.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// A one-component JP2 file holding `cs` with its last tile-part's `Psot`
/// set to 0 and no EOC, then an `xml ` box.
fn jp2_psot0(cs: &[u8]) -> Vec<u8> {
    let word = |at: usize| u32::from_be_bytes(cs[at..at + 4].try_into().unwrap());
    let mut ihdr = (word(12) - word(20)).to_be_bytes().to_vec();
    ihdr.extend_from_slice(&(word(8) - word(16)).to_be_bytes());
    ihdr.extend_from_slice(&cs[40..42]);
    ihdr.extend_from_slice(&[cs[42], 7, 0, 0]);
    let mut colr = vec![1, 0, 0];
    colr.extend_from_slice(&17u32.to_be_bytes());
    let mut jp2h = jp2_box(b"ihdr", &ihdr);
    jp2h.extend(jp2_box(b"colr", &colr));
    // Follow the tile-part chain to the last SOT.
    let mut sot = cs.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    while sot + word(sot + 6) as usize + 2 < cs.len() {
        sot += word(sot + 6) as usize;
    }
    let mut body = cs[..sot + 6].to_vec();
    body.extend_from_slice(&[0; 4]);
    body.extend_from_slice(&cs[sot + 10..cs.len() - 2]);
    let mut out = jp2_box(b"jP  ", b"\x0d\x0a\x87\x0a");
    out.extend(jp2_box(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    out.extend(jp2_box(b"jp2h", &jp2h));
    out.extend(jp2_box(b"jp2c", &body));
    out.extend(jp2_box(b"xml ", b"<a/>"));
    out
}

fn xml(metadata: Option<&rust_j2k::Jp2Metadata>) -> usize {
    metadata.unwrap().xml.len()
}

/// `read_header` walks a `Psot = 0` tile's packets to find where its
/// codestream ends, and that walk makes the same check. When the volumes
/// cover the tile, the packets end at the box and both calls report the
/// `xml ` box after it. When they leave packets out, the walk fails as the
/// decode does, so the codestream runs to the end of the file, as for any
/// tile that walk cannot finish: `read_header` reports no `xml ` box, and
/// the decode refuses the missing packets, before the bytes after them.
#[test]
fn read_header_s_walk_makes_the_same_check() {
    let covered = jp2_psot0(&read("poc_res_covered.j2k"));
    let image = decode(&covered).unwrap();
    assert_eq!(xml(image.jp2.as_ref()), 1);
    assert_eq!(xml(read_header(&covered).unwrap().jp2.as_ref()), 1);

    let missing = jp2_psot0(&read("poc_res_missing.j2k"));
    assert_eq!(xml(read_header(&missing).unwrap().jp2.as_ref()), 0);
    assert_refused("poc_res_missing.j2k as JP2", &missing, 0, 1);
}
