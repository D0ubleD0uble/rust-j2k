//! The auto traits every public type holds, checked at compile time.
//!
//! A caller moves an `Image` to another thread, shares a `Header` or a
//! `DecodeOptions` between threads, and holds any of them across a
//! `catch_unwind`. A private field that is `Rc`, a `Cell` or a raw pointer
//! would silently drop one of these, so adding such a field fails here
//! instead. `IncrementalDecoder` has the same check beside its definition.

#[cfg(feature = "__fuzz")]
use rust_j2k::fuzz::RepeatWork;
use rust_j2k::{
    AmountUnit, Association, ChannelDefinition, ChannelDefinitionEntry, ChannelMapping,
    ChannelType, ColorOutput, ColorSpace, ColorSpecification, Component, ComponentInfo,
    DecodeOptions, DecodeReport, Error, ErrorDetail, FileType, Header, Image, ImageHeader,
    IncrementalDecoder, IprBox, Jp2Metadata, LimitDetail, LimitKind, MappingKind, Palette,
    PaletteColumn, Raise, Ratio, ReaderRequirements, Refined, RefusedAt, Region, ReportEntry, Res,
    Resolution, SampleLimit, StandardFeature, TileGrid, UuidBox, UuidInfo, VendorFeature, Waiting,
    Window, WorkLimit, XmlBox,
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
    holds::<AmountUnit>();
    holds::<DecodeOptions>();
    holds::<DecodeReport>();
    holds::<ReportEntry>();
    holds::<Window>();
    holds::<Region>();
    holds::<WorkLimit>();
    holds::<SampleLimit>();
    holds::<Raise>();
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
    #[cfg(feature = "__fuzz")]
    holds::<RepeatWork>();
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

// Every public type in the crate, re-exported or defined, must be listed in
// `holds`, so a new type cannot skip the check. The functions, constants and
// the `Result` alias are not types with fields. The scan starts at `lib.rs`
// and follows each `pub mod x;` into its file, so the `fuzz` module is covered.

/// Drops comment lines and `#[...]` / `#![...]` attributes, so neither can
/// hide an item or fake one.
fn strip_noise(src: &str) -> String {
    let lines: String = src
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(|line| [line, "\n"])
        .collect();
    let chars: Vec<char> = lines.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let attr_at = match (chars[i], chars.get(i + 1), chars.get(i + 2)) {
            ('#', Some('['), _) => Some(i + 2),
            ('#', Some('!'), Some('[')) => Some(i + 3),
            _ => None,
        };
        let Some(mut j) = attr_at else {
            out.push(chars[i]);
            i += 1;
            continue;
        };
        let (mut depth, mut in_str) = (1, false);
        while j < chars.len() && depth > 0 {
            match chars[j] {
                '"' => in_str = !in_str,
                '[' if !in_str => depth += 1,
                ']' if !in_str => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        i = j;
    }
    out
}

fn idents(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty())
}

/// Each place `word` starts an item: not the tail of a longer identifier.
fn item_starts<'a>(src: &'a str, word: &'a str) -> impl Iterator<Item = &'a str> {
    src.match_indices(word).filter_map(move |(at, _)| {
        let before = src[..at].chars().next_back();
        let ok = before.is_none_or(|c| !c.is_alphanumeric() && c != '_');
        ok.then(|| &src[at + word.len()..])
    })
}

/// The type-like names a source file makes public: the names in each
/// `pub use` (a group may span lines) and the name after `pub struct`,
/// `enum`, `type`, `trait` and `union`, at any indent and after any item.
fn public_names(src: &str) -> Vec<String> {
    let src = strip_noise(src);
    let mut names = Vec::new();
    for rest in item_starts(&src, "pub use ") {
        let statement = rest.split(';').next().unwrap_or(rest);
        names.extend(idents(statement).map(str::to_owned));
    }
    for kind in ["struct", "enum", "type", "trait", "union"] {
        for rest in item_starts(&src, &format!("pub {kind} ")) {
            names.extend(idents(rest).take(1).map(str::to_owned));
        }
    }
    // Types are `CamelCase`; this drops paths, `as`, `self` and `CONSTANTS`.
    names.retain(|n| {
        n.starts_with(|c: char| c.is_ascii_uppercase()) && n.chars().any(|c| c.is_lowercase())
    });
    names
}

/// The names of the `pub mod x;` declarations (not inline modules).
fn public_mod_files(src: &str) -> Vec<String> {
    let src = strip_noise(src);
    item_starts(&src, "pub mod ")
        .filter_map(|rest| {
            let end = rest.find([';', '{'])?;
            rest[end..]
                .starts_with(';')
                .then(|| rest[..end].trim().to_owned())
        })
        .collect()
}

/// `names` of `src` plus those of every `pub mod` file below `dir`.
fn crate_names(dir: &std::path::Path, src: &str, out: &mut Vec<String>) {
    out.extend(public_names(src));
    for module in public_mod_files(src) {
        let file = dir.join(format!("{module}.rs"));
        let (path, child_dir) = if file.exists() {
            (file, dir.join(&module))
        } else {
            (dir.join(&module).join("mod.rs"), dir.join(&module))
        };
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("`pub mod {module}` not found at {}: {e}", path.display()));
        crate_names(&child_dir, &text, out);
    }
}

/// Whether `this` has a live `holds::<name>()` line, not a commented one.
fn is_listed(this: &str, name: &str) -> bool {
    strip_noise(this).contains(&format!("holds::<{name}>()"))
}

#[test]
fn every_public_type_is_listed() {
    let this = include_str!("auto_traits.rs");
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let lib = std::fs::read_to_string(src.join("lib.rs")).expect("read src/lib.rs");
    let mut names = Vec::new();
    crate_names(&src, &lib, &mut names);
    assert!(names.len() > 40, "the scan found too little: {names:?}");
    let missing: Vec<&String> = names
        .iter()
        .filter(|n| *n != "Result" && !is_listed(this, n))
        .collect();
    assert!(missing.is_empty(), "add to `holds`: {missing:?}");
}

#[test]
fn scanner_finds_each_shape() {
    let found = |src: &str, name: &str| {
        let names = public_names(src);
        assert!(names.iter().any(|n| n == name), "{name} not in {names:?}");
    };
    found("#[doc(hidden)] pub use a::AttrUse;", "AttrUse");
    found(
        "#[cfg(feature = \"x\")]\n#[doc(hidden)]\npub use a::CfgUse;",
        "CfgUse",
    );
    found("fn f() {}\npub use a::AfterFn;", "AfterFn");
    found("mod m {}\npub use a::AfterMod;", "AfterMod");
    found("pub trait Tr {}", "Tr");
    found("pub union Un { a: u8 }", "Un");
    found("mod m {\n    pub struct Indented;\n}", "Indented");
    found("#[derive(Debug)]\npub enum Derived { A }", "Derived");
    found("pub type Alias = u8;", "Alias");
    let group = "pub use a::{\n    B,\n    // a note\n    Cc, Dd,\n};";
    for name in ["Cc", "Dd"] {
        found(group, name);
    }
    // The `fuzz` module once gated behind a feature.
    let fuzz = "#[cfg(feature = \"__fuzz\")]\n#[doc(hidden)]\npub mod fuzz;";
    assert_eq!(public_mod_files(fuzz), ["fuzz"]);
    found(
        "#[cfg(feature = \"__fuzz\")]\n#[derive(Clone)]\npub struct RepeatWork { pub a: u64 }",
        "RepeatWork",
    );
}

#[test]
fn scanner_skips_what_is_not_a_public_type() {
    let src = "// pub struct Commented;\n/// pub use a::Doc;\n#[doc = \"pub struct InAttr;\"]\n\
               pub(crate) struct Crate;\npub fn f() {}\npub const MAX_THING: u8 = 1;\n\
               pub use a::{b, MAX_OTHER};\nstruct Private;\npub mod inline { }\n";
    assert_eq!(public_names(src), Vec::<String>::new());
    assert!(public_mod_files(src).is_empty());
    assert!(!is_listed("// holds::<Gone>();", "Gone"));
    assert!(is_listed("holds::<Here>(); // x", "Here"));
}
