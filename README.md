# delta-kernel-rs: into the Wasm

This is an experimental Wasm-compatible fork of the delta-kernel-rs.

This is intened to be used together with duckdb_delta extension when compiled to duckdb-wasm targets.

## Building
To get started, install Rust via [rustup], clone the repository, and then run:

```sh
RUSTUP_TOOLCHAIN=stable cargo build --package delta_kernel_ffi --features sync-engine --target wasm32-unknown-emscripten --release
```
