#![allow(dead_code)]
fn be16(v: u16) -> [u8; 2] { v.to_be_bytes() }
#[path = "support/synth.rs"]
mod synth;
fn old0(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(8 + payload.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
fn old1(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
fn old2(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
fn old3(marker: u16, body: &[u8]) -> Vec<u8> {
    let mut s = marker.to_be_bytes().to_vec();
    s.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
    s.extend_from_slice(body);
    s
}
fn old4(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
fn old5(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
    out.extend_from_slice(body);
    out
}
fn old6(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&(2 + body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}
fn old7(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(8 + payload.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
fn old8(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&(2 + body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}
fn old9(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&(2 + body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}
fn old10(marker: u16, body: &[u8]) -> Vec<u8> {
    let mut out = marker.to_be_bytes().to_vec();
    out.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
    out.extend_from_slice(body);
    out
}
fn old11(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&(2 + body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}
fn old12(marker: u16, body: &[u8]) -> Vec<u8> {
    let mut s = marker.to_be_bytes().to_vec();
    s.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
    s.extend_from_slice(body);
    s
}
fn old13(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
fn old14(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
fn old15(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let length = u32::try_from(8 + payload.len()).expect("test box fits LBox");
    let mut out = length.to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
fn old16(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
fn old17(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}
fn old18(m: u16, body: &[u8]) -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(&be16(m));
    s.extend_from_slice(&be16((body.len() + 2) as u16));
    s.extend_from_slice(body);
    s
}

#[test]
fn identical() {
    let kinds: [&[u8;4]; 3] = [b"jp2c", b"colr", b"\0\0\0\0"];
    let mut bodies: Vec<Vec<u8>> = vec![vec![]];
    for n in [1usize, 2, 7, 300, 4000, 65000] { bodies.push((0..n).map(|i| (i * 31 + n) as u8).collect()); }
    for b in &bodies {
        for k in kinds { assert_eq!(old0(k, b), synth::bx(k, b)); }
        for k in kinds { assert_eq!(old1(k, b), synth::bx(k, b)); }
        for k in kinds { assert_eq!(old2(k, b), synth::bx(k, b)); }
        for c in [0xFF51u16, 0xFF52, 0xFF90, 0] { assert_eq!(old3(c, b), synth::seg(c, b)); }
        for k in kinds { assert_eq!(old4(k, b), synth::bx(k, b)); }
        for c in [0xFF51u16, 0xFF52, 0xFF90, 0] { assert_eq!(old5(c, b), synth::seg(c, b)); }
        for c in [0xFF51u16, 0xFF52, 0xFF90, 0] { assert_eq!(old6(c, b), synth::seg(c, b)); }
        for k in kinds { assert_eq!(old7(k, b), synth::bx(k, b)); }
        for c in [0xFF51u16, 0xFF52, 0xFF90, 0] { assert_eq!(old8(c, b), synth::seg(c, b)); }
        for c in [0xFF51u16, 0xFF52, 0xFF90, 0] { assert_eq!(old9(c, b), synth::seg(c, b)); }
        for c in [0xFF51u16, 0xFF52, 0xFF90, 0] { assert_eq!(old10(c, b), synth::seg(c, b)); }
        for c in [0xFF51u16, 0xFF52, 0xFF90, 0] { assert_eq!(old11(c, b), synth::seg(c, b)); }
        for c in [0xFF51u16, 0xFF52, 0xFF90, 0] { assert_eq!(old12(c, b), synth::seg(c, b)); }
        for k in kinds { assert_eq!(old13(k, b), synth::bx(k, b)); }
        for k in kinds { assert_eq!(old14(k, b), synth::bx(k, b)); }
        for k in kinds { assert_eq!(old15(k, b), synth::bx(k, b)); }
        for k in kinds { assert_eq!(old16(k, b), synth::bx(k, b)); }
        for k in kinds { assert_eq!(old17(k, b), synth::bx(k, b)); }
        for c in [0xFF51u16, 0xFF52, 0xFF90, 0] { assert_eq!(old18(c, b), synth::seg(c, b)); }
    }
}
