# Examples

Run each one from the repository root with `cargo run --example <name>`. The
commands use fixtures from `tests/fixtures/`.

`decode` decodes a file and writes a PGM (one channel) or PPM (three
channels):

```sh
cargo run --example decode -- tests/fixtures/multicomponent_lossless.j2k out.ppm
```

`info` prints a file's header: size, components, tile grid, resolution levels,
quality layers, and the JP2 metadata:

```sh
cargo run --example info -- tests/fixtures/jp2_metadata_boxes.jp2
```

`untrusted` decodes a file from a source you don't trust. It uses the
default limits, prints the details of a limit refusal, and retries past it
only with `--allow-raise`, by a factor of at most 4. A cut-short file is
decoded again with `strict` off, and the example prints what is missing:

```sh
cargo run --example untrusted -- \
    tests/fixtures/psot0/single_psot0_no_eoc_trunc100.j2k
cargo run --example untrusted -- --allow-raise \
    tests/fixtures/tier1_work_bound/t1bomb_1024_64_88.j2k
```

`grib2` decodes the codestream from a GRIB2 message's section 7 and scales it
with R, E and D from section 5, Y = (R + X × 2^E) / 10^D. It prints the
smallest and largest value and the first few:

```sh
cargo run --example grib2 -- \
    tests/fixtures/jpeg2000_regular_latlon.j2k 270.466796875 -10 0
```

`common/pnm.rs` is the PGM and PPM writer the examples share. It is not an
example.
