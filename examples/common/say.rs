//! `say!` and `say_err!`, a `println!` and `eprintln!` that survive a closed
//! pipe, for the examples.
//!
//! `example | head -n 1` closes the pipe after one line. Rust ignores
//! SIGPIPE, so the next `println!` fails with `BrokenPipe` and panics.
//! These macros ignore that one error, so the run goes on and ends with the
//! status it would have had: a closed pipe is not a failure, and it
//! doesn't hide one.
//!
//! The examples include this file with `#[path]`. It is not an example
//! itself: Cargo only builds `examples/*.rs` and `examples/*/main.rs`.

/// Print a line to stdout like `println!`, and ignore a closed pipe. Ends
/// the process with status 1 on any other write error.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let wrote = writeln!(std::io::stdout().lock(), $($arg)*);
        if let Err(e) = wrote {
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                say_err!("cannot write to stdout: {e}");
                std::process::exit(1);
            }
        }
    }};
}

/// Print a line to stderr like `eprintln!`, and ignore any write error:
/// there is nowhere left to report one.
macro_rules! say_err {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stderr().lock(), $($arg)*);
    }};
}

pub(crate) use {say, say_err};
