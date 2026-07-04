# area_example_rust

A 100-line Rust area plugin. Use as a starting template.

## Layout

```text
Cargo.toml
src/main.rs               # the HTTP/JSON plugin server
package/
    manifest.toml         # runtime = "rust", entry = "area_example_rust"
    area.toml
    profiles/twr.toml
```

The host expects the compiled binary at `package/plugin/<entry>`. Build
and copy it there before installing or tarring:

```bash
cargo build --release --bin area_example_rust
mkdir -p package/plugin
cp ../../target/release/area_example_rust package/plugin/
```

## Install locally

```bash
ln -s "$PWD/package" "$HOME/.local/share/es_runway_selector/areas/example-rust"
es_runway_selector area list                  # should show example-rust
es_runway_selector                             # runs a full cycle
```

## Package for the registry

```bash
cd package
tar -czf ../area-example-rust-0.1.0.tar.gz .
sha256sum ../area-example-rust-0.1.0.tar.gz
```

Ship one tarball per `(target OS, arch)` since the binary inside is
native code.
