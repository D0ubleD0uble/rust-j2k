#![no_main]
//! Fuzz the raw MQ arithmetic decoder over arbitrary coded bytes: state-table
//! transitions, BYTEIN stuffing and marker handling, and bounded past-end
//! synthesis, via the `--cfg fuzzing` hook the library exposes.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    rust_j2k::fuzz::mq_stream(data);
});
