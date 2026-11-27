//! Run an example through `cargo run`, with a deadline.
//!
//! The tests of the examples include this file with `#[path]`. Cargo gives
//! a test no path to an example, so the nested build compiles the example
//! itself; `--profile test` reuses the library the tests already built. The
//! run follows `CARGO_TARGET_DIR` from the environment, but not a
//! `--target-dir` flag given to the outer `cargo test`. A run past
//! [`DEADLINE`], such as an example caught in a loop, is killed and fails the
//! test instead of hanging it.
//!
//! The runs in one test binary take turns. Two `cargo run`s at once can
//! race: one can replace the example binary while the other starts it, and
//! the start then fails with "No such file or directory".

// Each test binary uses some of these functions.
#![allow(dead_code)]

use std::ffi::OsStr;
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// How long one run may take, the nested build included.
pub const DEADLINE: Duration = Duration::from_secs(300);

/// Held for each run, so the runs take turns.
static TURN: Mutex<()> = Mutex::new(());

/// Run `example` with `args` and collect what it prints.
pub fn run_example(example: &str, args: &[&OsStr]) -> Output {
    run(example, args, false, false)
}

/// Run `example` with its stdout a pipe whose read end is already closed, as
/// `example | head -n 1` leaves it after the first line. `Output::stdout` is
/// empty.
pub fn run_example_closed_stdout(example: &str, args: &[&OsStr]) -> Output {
    run(example, args, true, false)
}

/// As [`run_example_closed_stdout`], with stderr closed too, as
/// `example 2>&1 | head -n 1` leaves them. Both outputs are empty.
pub fn run_example_closed_output(example: &str, args: &[&OsStr]) -> Output {
    run(example, args, true, true)
}

/// A pipe nobody reads: a write to it fails with `BrokenPipe`.
fn closed_pipe() -> Stdio {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    Stdio::from(writer)
}

fn run(example: &str, args: &[&OsStr], out: bool, err: bool) -> Output {
    // A test that failed while holding it leaves it poisoned; the next run
    // is still safe.
    let _turn = TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    if out || err {
        // Build with stderr piped first. The run below may have no stderr
        // to show a compiler message, and cargo would then exit 101 with
        // no diagnostic.
        let build = Command::new(&cargo)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["build", "--quiet", "--offline", "--profile", "test"])
            .args(["--example", example])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let message = String::from_utf8_lossy(&build.stderr);
        // A warning counts too: `cargo run` repeats it on its stderr, which
        // the run below may not have.
        let clean = build.status.success() && message.is_empty();
        assert!(clean, "building {example}: {message}");
    }
    let mut child = Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["run", "--quiet", "--offline", "--profile", "test"])
        .args(["--example", example, "--"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(if out { closed_pipe() } else { Stdio::piped() })
        .stderr(if err { closed_pipe() } else { Stdio::piped() })
        .spawn()
        .unwrap();
    // Read both pipes while the child runs, so a full pipe can't stall it.
    let drain = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).unwrap();
            bytes
        })
    };
    let stdout = child.stdout.take().map(|pipe| drain(Box::new(pipe)));
    let stderr = child.stderr.take().map(|pipe| drain(Box::new(pipe)));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the {example} example ran past {DEADLINE:?}");
        }
        thread::sleep(Duration::from_millis(20));
    };
    Output {
        status,
        stdout: stdout.map_or_else(Vec::new, |t| t.join().unwrap()),
        stderr: stderr.map_or_else(Vec::new, |t| t.join().unwrap()),
    }
}
