//! The extension's process: the node starts it as extension.toml's `[run]` says (`bin/hello`, from
//! the extension's folder) and talks to it over stdin and stdout (the SDK handles that). Log with
//! `h.log(…)`: stdout is the node's.

use pointman_extension::protocol::output_request::Role;
use pointman_extension::{Extension, Job};
use serde_json::json;

// proto/hello/v1/hello.proto's messages, made by build.rs
mod proto {
    pointman_extension::include_proto!();
}
use proto::hello::v1::{GreetParams, GreetResult, Settings};

fn main() -> anyhow::Result<()> {
    let mut ext = Extension::new()?;
    let h = ext.handle();

    // hello.greet: its params are GreetParams, its result GreetResult (`<Verb>Params`, `<Verb>Result`)
    ext.typed_command(move |job: &Job, p: GreetParams| {
        let settings: Settings = h.typed_settings()?;
        let text = format!("{}, {}!", settings.greeting, p.name);
        job.progress(0.5, "writing");
        let out = job.work.join("greeting.txt");
        std::fs::write(&out, format!("{text}\n"))?;
        job.output(&out, "text", Role::Main, json!({})); // posted where the request came from
        h.event("hello.last", json!({"name": p.name}));
        Ok(GreetResult { text, ..Default::default() })
    });

    ext.run()
}
