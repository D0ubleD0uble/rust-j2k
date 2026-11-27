//! Run an example through `cargo run`, with a deadline.
//!
//! `tests/examples_untrusted.rs` and `tests/examples_grib2.rs` include this
//! file with `#[path]`. Cargo gives a test no path to an example, so the
//! nested build compiles the example itself; `--profile test` reuses the
//! library the tests already built. The run follows `CARGO_TARGET_DIR` from
//! the environment, but not a `--target-dir` flag given to the outer
//! `cargo test`. A run past [`DEADLINE`], such as an example caught in a
//! loop, is killed and fails the test instead of hanging it.
//!
//! The runs in one test binary take turns. Two `cargo run`s at once can
//! race: one can replace the example binary while the other starts it, and
//! the start then fails with "No such file or directory".

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
    // A test that failed while holding it leaves it poisoned; the next run
    // is still safe.
    let _turn = TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut child = Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["run", "--quiet", "--offline", "--profile", "test"])
        .args(["--example", example, "--"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
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
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    }
}
