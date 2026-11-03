//! extension.toml's schemas are made from proto/hello/v1/hello.proto: this fails while they say something else. To
//! write them: `pointman-extension manifest write` (pointman-extension's README says how to install it).

#[test]
fn extension_toml_matches_the_proto() {
    if let Err(e) = pointman_extension::proto::check(env!("CARGO_MANIFEST_DIR").as_ref()) {
        panic!("{e:#}");
    }
}
