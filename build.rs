//! The Rust types for proto/hello/v1/hello.proto (pointman-extension's `proto` feature): src/main.rs takes them in
//! with `include_proto!`. It warns while extension.toml's schemas are stale; tests/manifest.rs fails.

fn main() {
    if let Err(e) = pointman_extension::proto::build() {
        panic!("{e:#}");
    }
}
