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
    ReaderRequirements, Region, ReportEntry, Res, Resolution, StandardFeature, TileGrid, UuidBox,
    UuidInfo, VendorFeature, Window, WorkLimit, XmlBox,
};

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
    holds::<DecodeOptions>();
    holds::<DecodeReport>();
    holds::<ReportEntry>();
    holds::<Window>();
    holds::<Region>();
    holds::<WorkLimit>();
    holds::<ColorOutput>();
    holds::<IncrementalDecoder>();
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

// `LimitKind` is a plain value a caller can copy, compare and key a map by.
const fn plain<T: Copy + Eq + std::hash::Hash + std::fmt::Debug>() {}
const _: () = plain::<LimitKind>();

// The assertions are all compile-time; this keeps the file a test target.
#[test]
fn public_types_are_send_sync_and_unwind_safe() {}
