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

`thumbnail` decodes at a smaller size, and optionally only a window, with
`with_resolution_reduction` and `with_region`. The first command drops 2
levels (a quarter of each side), the second drops 1 level of a window:

```sh
cargo run --example thumbnail -- \
    tests/fixtures/precincts_varied_sizes_lossless.j2k 2 thumb.ppm
cargo run --example thumbnail -- \
    tests/fixtures/precincts_varied_sizes_lossless.j2k 1 16 8 112 88 part.ppm
```

`stream` feeds a file to `IncrementalDecoder` in chunks and prints each
refine as the bytes arrive:

```sh
cargo run --example stream -- tests/fixtures/layers/lrcp.j2k 100
```

`common/pnm.rs` is the PGM and PPM writer the examples share. It is not an
example.
