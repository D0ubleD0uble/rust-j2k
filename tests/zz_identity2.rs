#![allow(dead_code)]
use std::path::PathBuf;
#[path = "support/synth.rs"]
mod synth;
mod m0_old { use std::path::{Path,PathBuf}; pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/window")
}

pub fn read(name: &str) -> Vec<u8> {
    let path = dir().join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

 }
mod m0_new { use std::path::{Path,PathBuf}; use super::synth; pub fn dir() -> PathBuf {
    synth::fixtures("window")
}

pub fn read(name: &str) -> Vec<u8> {
    synth::read_file(dir().join(name))
}

 }
#[test]
fn t0() {
    assert_eq!(m0_old::dir(), m0_new::dir());
    for e in std::fs::read_dir(m0_old::dir()).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap(); assert_eq!(m0_old::read(&n), m0_new::read(&n)); } }
}
mod m1_old { use std::path::{Path,PathBuf}; pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ht_vlc_stuffing")
}

pub fn read(name: &str, tag: &str) -> Vec<u8> {
    let path = dir().join(format!("{name}.{tag}.j2k"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

 }
mod m1_new { use std::path::{Path,PathBuf}; use super::synth; pub fn dir() -> PathBuf {
    synth::fixtures("ht_vlc_stuffing")
}

pub fn read(name: &str, tag: &str) -> Vec<u8> {
    synth::read_file(dir().join(format!("{name}.{tag}.j2k")))
}

 }
#[test]
fn t1() {
    assert_eq!(m1_old::dir(), m1_new::dir());
    for e in std::fs::read_dir(m1_old::dir()).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap(); if let Some(n)=n.strip_suffix(".j2k") { if let Some((a,b))=n.split_once('.') { assert_eq!(m1_old::read(a,b), m1_new::read(a,b)); } } } }
}
mod m2_old { use std::path::{Path,PathBuf}; pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub fn read(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

 }
mod m2_new { use std::path::{Path,PathBuf}; use super::synth; pub fn dir() -> PathBuf {
    synth::fixtures("")
}

pub fn read(name: &str) -> Vec<u8> {
    synth::read_file(dir().join(name))
}

 }
#[test]
fn t2() {
    assert_eq!(m2_old::dir(), m2_new::dir());
    for e in std::fs::read_dir(m2_old::dir()).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap(); assert_eq!(m2_old::read(&n), m2_new::read(&n)); } }
}
mod m3_old { use std::path::{Path,PathBuf}; pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/poc_coverage")
}

pub fn read(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

 }
mod m3_new { use std::path::{Path,PathBuf}; use super::synth; pub fn dir() -> PathBuf {
    synth::fixtures("poc_coverage")
}

pub fn read(name: &str) -> Vec<u8> {
    synth::read_file(dir().join(name))
}

 }
#[test]
fn t3() {
    assert_eq!(m3_old::dir(), m3_new::dir());
    for e in std::fs::read_dir(m3_old::dir()).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap(); assert_eq!(m3_old::read(&n), m3_new::read(&n)); } }
}
mod m4_old { use std::path::{Path,PathBuf}; pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/poc_precedence")
}

 }
mod m4_new { use std::path::{Path,PathBuf}; use super::synth; pub fn dir() -> PathBuf {
    synth::fixtures("poc_precedence")
}

 }
#[test]
fn t4() {
    assert_eq!(m4_old::dir(), m4_new::dir());
    for e in std::fs::read_dir(m4_old::dir()).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap();  } }
}
mod m5_old { use std::path::{Path,PathBuf}; pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/psot0")
}

 }
mod m5_new { use std::path::{Path,PathBuf}; use super::synth; pub fn dir() -> PathBuf {
    synth::fixtures("psot0")
}

 }
#[test]
fn t5() {
    assert_eq!(m5_old::dir(), m5_new::dir());
    for e in std::fs::read_dir(m5_old::dir()).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap();  } }
}
mod m6_old { use std::path::{Path,PathBuf}; pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/roi_bypass")
}

 }
mod m6_new { use std::path::{Path,PathBuf}; use super::synth; pub fn dir() -> PathBuf {
    synth::fixtures("roi_bypass")
}

 }
#[test]
fn t6() {
    assert_eq!(m6_old::dir(), m6_new::dir());
    for e in std::fs::read_dir(m6_old::dir()).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap();  } }
}
mod m7_old { use std::path::{Path,PathBuf}; pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/work_bound")
}

 }
mod m7_new { use std::path::{Path,PathBuf}; use super::synth; pub fn dir() -> PathBuf {
    synth::fixtures("work_bound")
}

 }
#[test]
fn t7() {
    assert_eq!(m7_old::dir(), m7_new::dir());
    for e in std::fs::read_dir(m7_old::dir()).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap();  } }
}
mod m8_old { use std::path::{Path,PathBuf}; pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jp2_real")
}

 }
mod m8_new { use std::path::{Path,PathBuf}; use super::synth; pub fn dir() -> PathBuf {
    synth::fixtures("jp2_real")
}

 }
#[test]
fn t8() {
    assert_eq!(m8_old::dir(), m8_new::dir());
    for e in std::fs::read_dir(m8_old::dir()).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap();  } }
}
mod m9_old { use std::path::{Path,PathBuf}; pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

 }
mod m9_new { use std::path::{Path,PathBuf}; use super::synth; pub fn dir() -> PathBuf {
    synth::fixtures("")
}

 }
#[test]
fn t9() {
    assert_eq!(m9_old::dir(), m9_new::dir());
    for e in std::fs::read_dir(m9_old::dir()).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap();  } }
}
mod m10_old { use std::path::{Path,PathBuf}; pub fn read(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/packed_overrun")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

 }
mod m10_new { use std::path::{Path,PathBuf}; use super::synth; pub fn read(name: &str) -> Vec<u8> {
    synth::read_file(synth::fixtures("packed_overrun").join(name))
}

 }
#[test]
fn t10() {
    for e in std::fs::read_dir(synth::fixtures("packed_overrun")).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap(); assert_eq!(m10_old::read(&n), m10_new::read(&n)); } }
}
mod m11_old { use std::path::{Path,PathBuf}; pub fn read(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tile_header")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

 }
mod m11_new { use std::path::{Path,PathBuf}; use super::synth; pub fn read(name: &str) -> Vec<u8> {
    synth::read_file(synth::fixtures("tile_header").join(name))
}

 }
#[test]
fn t11() {
    for e in std::fs::read_dir(synth::fixtures("tile_header")).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap(); assert_eq!(m11_old::read(&n), m11_new::read(&n)); } }
}
mod m12_old { use std::path::{Path,PathBuf}; pub fn read(path: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

 }
mod m12_new { use std::path::{Path,PathBuf}; use super::synth; pub fn read(path: &str) -> Vec<u8> {
    synth::read_file(synth::fixtures("").join(path))
}

 }
#[test]
fn t12() {
    for e in std::fs::read_dir(synth::fixtures("")).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap(); assert_eq!(m12_old::read(&n), m12_new::read(&n)); } }
}
mod m13_old { use std::path::{Path,PathBuf}; pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub fn read(path: &str) -> Vec<u8> {
    let path = fixtures().join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

 }
mod m13_new { use std::path::{Path,PathBuf}; use super::synth; pub fn fixtures() -> PathBuf {
    synth::fixtures("")
}

pub fn read(path: &str) -> Vec<u8> {
    synth::read_file(fixtures().join(path))
}

 }
#[test]
fn t13() {
    assert_eq!(m13_old::fixtures(), m13_new::fixtures());
    for e in std::fs::read_dir(m13_old::fixtures()).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap(); assert_eq!(m13_old::read(&n), m13_new::read(&n)); } }
}
mod m14_old { use std::path::{Path,PathBuf}; pub fn read(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.jp2"));
    std::fs::read(path).unwrap()
}

 }
mod m14_new { use std::path::{Path,PathBuf}; use super::synth; pub fn read(name: &str) -> Vec<u8> {
    synth::read_file(synth::fixtures("").join(format!("{name}.jp2")))
}

 }
#[test]
fn t14() {
    for e in std::fs::read_dir(synth::fixtures("")).unwrap() { let e=e.unwrap(); if e.path().is_file() { let n=e.file_name().into_string().unwrap(); if let Some(a)=n.strip_suffix(".jp2") { assert_eq!(m14_old::read(a), m14_new::read(a)); } } }
}
