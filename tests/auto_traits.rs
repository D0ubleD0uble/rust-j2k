//! The auto traits every public type holds, checked at compile time.
//!
//! A caller moves an `Image` to another thread, shares a `Header` or a
//! `DecodeOptions` between threads, and holds any of them across a
//! `catch_unwind`. A private field that is `Rc`, a `Cell` or a raw pointer
//! would silently drop one of these, so adding such a field fails here
//! instead. `IncrementalDecoder` has the same check beside its definition.

use rust_j2k::{
    Association, ChannelDefinition, ChannelDefinitionEntry, ChannelMapping, ChannelType,
    ColorOutput, ColorSpace, ColorSpecification, Component, ComponentInfo, DecodeOptions,
    DecodeReport, Error, ErrorDetail, FileType, Header, Image, ImageHeader, IncrementalDecoder,
    IprBox, Jp2Metadata, LimitDetail, LimitKind, MappingKind, Palette, PaletteColumn, Ratio,
    ReaderRequirements, Refined, RefusedAt, Region, ReportEntry, Res, Resolution, StandardFeature,
    TileGrid, UuidBox, UuidInfo, VendorFeature, Waiting, Window, WorkLimit, XmlBox,
};

// Used only in `const _` blocks, which Rust 1.87 does not count as a use.
#[allow(dead_code)]
const fn holds<T: Send + Sync + std::panic::UnwindSafe + std::panic::RefUnwindSafe>() {}

const _: () = {
    holds::<Image>();
    holds::<Component>();
    holds::<Header>();
    holds::<ComponentInfo>();
    holds::<TileGrid>();
    holds::<Error>();
    holds::<ErrorDetail>();
    holds::<LimitDetail>();
    holds::<LimitKind>();
    holds::<RefusedAt>();
    holds::<DecodeOptions>();
    holds::<DecodeReport>();
    holds::<ReportEntry>();
    holds::<Window>();
    holds::<Region>();
    holds::<WorkLimit>();
    holds::<ColorOutput>();
    holds::<IncrementalDecoder>();
    holds::<Refined>();
    holds::<Waiting>();
    holds::<Jp2Metadata>();
    holds::<ImageHeader>();
    holds::<ColorSpecification>();
    holds::<ColorSpace>();
    holds::<FileType>();
    holds::<ReaderRequirements>();
    holds::<StandardFeature>();
    holds::<VendorFeature>();
    holds::<Resolution>();
    holds::<Res>();
    holds::<Ratio>();
    holds::<XmlBox>();
    holds::<IprBox>();
    holds::<UuidBox>();
    holds::<UuidInfo>();
    holds::<ChannelDefinition>();
    holds::<ChannelDefinitionEntry>();
    holds::<ChannelType>();
    holds::<Association>();
    holds::<Palette>();
    holds::<PaletteColumn>();
    holds::<ChannelMapping>();
    holds::<MappingKind>();
};

// `LimitKind`, `RefusedAt` and `TileGrid` are plain values a caller can copy, compare
// and key a map by.
// Used only in `const _` blocks, which Rust 1.87 does not count as a use.
#[allow(dead_code)]
const fn plain<T: Copy + Eq + std::hash::Hash + std::fmt::Debug>() {}
const _: () = {
    plain::<LimitKind>();
    plain::<RefusedAt>();
    plain::<TileGrid>();
};

// The assertions are all compile-time; this keeps the file a test target.
#[test]
fn public_types_are_send_sync_and_unwind_safe() {}

// Every public type in `lib.rs`, re-exported or defined there, must be listed
// in `holds`, so a new type cannot skip the check. The functions and the
// `Result` alias are not types with fields.
#[test]
fn every_public_type_is_listed() {
    let this = include_str!("auto_traits.rs");
    // Drop comment lines, so a doc comment above a `pub use` cannot hide it.
    let lib: String = include_str!("../src/lib.rs")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(|line| [line, "\n"])
        .collect();
    let mut names: Vec<&str> = Vec::new();
    for statement in lib
        .split(';')
        .filter(|s| s.trim_start().starts_with("pub use "))
    {
        names.extend(statement.split(|c: char| !c.is_alphanumeric() && c != '_'));
    }
    // Types defined in `lib.rs` itself.
    for line in lib.lines() {
        for kind in ["pub struct ", "pub enum ", "pub type "] {
            if let Some(rest) = line.strip_prefix(kind) {
                names.extend(
                    rest.split(|c: char| !c.is_alphanumeric() && c != '_')
                        .take(1),
                );
            }
        }
    }
    let missing: Vec<&str> = names
        .into_iter()
        .filter(|n| n.starts_with(|c: char| c.is_ascii_uppercase()))
        .filter(|n| *n != "Result" && !this.contains(&format!("holds::<{n}>()")))
        .collect();
    assert!(missing.is_empty(), "add to `holds`: {missing:?}");
}
