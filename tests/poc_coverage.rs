//! A `POC` marker whose volumes leave out packets of the tile: the packets
//! they do not reach are missing from the codestream, and the decode refuses
//! it with [`Error::Codestream`] rather than decode them as empty.
//! `opj_compress` 2.5.0 writes only the packets its volumes reach, and
//! `opj_decompress` 2.5.0 decodes the rest as empty, so its image is not the
//! source. The files are written by `scripts/gen-poc-coverage-fixtures.py`;
//! see `tests/fixtures/poc_coverage/README.md`.

#[path = "support/synth.rs"]
mod synth;
use std::path::PathBuf;
use synth::bx;

use rust_j2k::{Error, decode, read_header};

fn dir() -> PathBuf {
    synth::fixtures("poc_coverage")
}

fn read(name: &str) -> Vec<u8> {
    synth::read_file(dir().join(name))
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
            m.message().contains("leave out")
                && m.message()
                    .contains(&format!("component {component}, resolution {resolution},")),
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

/// A one-component JP2 file holding `cs` with its last tile-part's `Psot`
/// set to 0 and no EOC, then an `xml ` box.
fn jp2_psot0(cs: &[u8]) -> Vec<u8> {
    let word = |at: usize| u32::from_be_bytes(cs[at..at + 4].try_into().unwrap());
    let ihdr = synth::ihdr_payload(
        word(12) - word(20),
        word(8) - word(16),
        u16::from_be_bytes([cs[40], cs[41]]),
        [cs[42], 7, 0, 0],
    );
    // Follow the tile-part chain to the last SOT.
    let mut sot = cs.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    while sot + word(sot + 6) as usize + 2 < cs.len() {
        sot += word(sot + 6) as usize;
    }
    let mut body = cs[..sot + 6].to_vec();
    body.extend_from_slice(&[0; 4]);
    body.extend_from_slice(&cs[sot + 10..cs.len() - 2]);
    synth::jp2_file(
        &[bx(b"ihdr", &ihdr), bx(b"colr", &synth::colr_enum(17))],
        &[],
        &body,
        &[bx(b"xml ", b"<a/>")],
    )
}

fn xml(metadata: Option<&rust_j2k::Jp2Metadata>) -> usize {
    metadata.unwrap().xml.len()
}

/// `read_header` walks a `Psot = 0` tile's packets to find where its
/// codestream ends. That walk does not check that the volumes cover the
/// tile: the packets they reach end at the box either way. So both files
/// end at their box, and `read_header` reports the `xml ` box after it. The
/// decode refuses the one whose volumes leave packets out, as it refuses
/// the raw codestream.
#[test]
fn read_header_reports_the_boxes_after_a_tile_with_packets_left_out() {
    let covered = jp2_psot0(&read("poc_res_covered.j2k"));
    let image = decode(&covered).unwrap();
    assert_eq!(xml(image.jp2.as_ref()), 1);
    assert_eq!(xml(read_header(&covered).unwrap().jp2.as_ref()), 1);

    let missing = jp2_psot0(&read("poc_res_missing.j2k"));
    assert_eq!(xml(read_header(&missing).unwrap().jp2.as_ref()), 1);
    assert_refused("poc_res_missing.j2k as JP2", &missing, 0, 1);
}
