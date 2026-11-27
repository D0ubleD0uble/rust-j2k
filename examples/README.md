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

`common/pnm.rs` is the PGM and PPM writer the examples share. It is not an
example.
