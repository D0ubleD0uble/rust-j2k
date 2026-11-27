# Examples

Run each one from the repository root with `cargo run --example <name>`. The
commands below use fixtures from `tests/fixtures/`.

- `decode`: decode a file and write a PGM (one channel) or PPM (three
  channels). `cargo run --example decode -- tests/fixtures/multicomponent_lossless.j2k out.ppm`
- `info`: print a file's header: size, components, tile grid, resolution
  levels, quality layers, and the JP2 metadata. `cargo run --example info -- tests/fixtures/jp2_metadata_boxes.jp2`

`common/pnm.rs` is the PGM and PPM writer the examples share. It is not an
example.
