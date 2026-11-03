# hello: a Pointman extension template

A compiled extension in Rust: one command, `hello.greet`, that writes a greeting to a file and reports
it. Make a repo from this template named `pointman-<id>`, with the topic `pointman-extension`, rename
`hello` everywhere (Cargo.toml, the id in extension.toml, the command, tool and event names, and the
folder, file and package of proto/hello/v1/hello.proto), and write your own commands in `src/main.rs`.
It's MIT, as every extension repo is by default.

- **The SDK:** [pointman-extension](https://github.com/pointman-ai/pointman-extension) speaks the
  node's protocol over stdin and stdout, so `src/main.rs` only declares commands and answers them.
- **Its own proto:** `proto/hello/v1/hello.proto` holds the command's params and result (`GreetParams`,
  `GreetResult`: `hello.<verb>` takes `<Verb>Params`) and the settings (`Settings`), with each field's doc and
  limits. `pointman-extension manifest write` makes extension.toml's `params` and `[settings]` from it, keeping the
  rest of the file (install it with `cargo install --git https://github.com/pointman-ai/pointman-extension
  --features proto pointman-extension`), and build.rs makes the Rust types `src/main.rs` takes. `tests/manifest.rs`
  fails while extension.toml is stale. It's the recommended way, not the only one: hand-written schemas in
  extension.toml still work (drop proto/, build.rs and tests/manifest.rs, and use `ext.command`).
- **Run the tests:** `cargo test`. They start the built binary the way a node does, against the SDK's
  stand-in node (`testing::StandIn`), which also answers secrets within the manifest's permissions.
- **Try it on a machine:** `cargo build --release`, copy `target/release/hello` to `bin/hello`, then
  `pointman add .` from this folder, and run its command from any thread.
- **Release it:** bump `version` in extension.toml, then push the tag (`git tag v0.1.1 && git push
  origin v0.1.1`). `release.yml` builds macOS (Apple silicon and Intel) and Linux (x86_64 and arm64,
  static), attaches `hello-<version>-<platform>.tar.gz` for each, and core takes the release into the
  workspace's catalogue. Machines install it with `pointman add hello` and move to new releases with
  `pointman upgrade`.

An extension in Python is still possible (Pointman's `sdk/template`), for one that wraps a Python app's
own API, as Resolve and Blender do.
