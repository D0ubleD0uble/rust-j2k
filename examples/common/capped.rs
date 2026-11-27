//! A capped list for printing, for the `untrusted` example.
//!
//! `examples/untrusted.rs` includes this file with `#[path]`, and so does
//! `tests/examples_untrusted.rs`, which checks the cap. It is not an example
//! itself: Cargo only builds `examples/*.rs` and `examples/*/main.rs`.

use std::fmt::Display;

/// How many items [`capped`] lists. A hostile file can carry many report
/// entries, so the example prints this many and counts the rest.
pub const SHOWN: usize = 16;

/// The first [`SHOWN`] items as lines, then "and N more" when there are
/// more.
pub fn capped<T: Display>(items: &[T]) -> Vec<String> {
    let mut lines: Vec<String> = items
        .iter()
        .take(SHOWN)
        .map(|item| item.to_string())
        .collect();
    let more = items.iter().skip(SHOWN).count();
    if more > 0 {
        lines.push(format!("and {more} more"));
    }
    lines
}
