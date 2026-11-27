//! `say!`, a `println!` that survives a closed stdout, for the examples.
//!
//! `example | head -n 1` closes the pipe after one line. Rust ignores
//! SIGPIPE, so the next `println!` fails with `BrokenPipe` and panics.
//! `say!` ignores that one error, so the run goes on and ends with the
//! status it would have had: a closed pipe is not a failure, and it
//! doesn't hide one. Any other write error ends the run with status 1.
//!
//! The examples include this file with `#[macro_use]` and `#[path]`. It is
//! not an example itself: Cargo only builds `examples/*.rs` and
//! `examples/*/main.rs`.

/// Print a line to stdout like `println!`, and ignore a closed pipe.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let wrote = writeln!(std::io::stdout().lock(), $($arg)*);
        if let Err(e) = wrote {
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                eprintln!("cannot write to stdout: {e}");
                std::process::exit(1);
            }
        }
    }};
}
