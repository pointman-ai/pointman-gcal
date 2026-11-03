//! Google on Pointman: Gmail (`mail`) and Google Calendar (`calendar`) for every agent, through Google's APIs
//! (`google`), with any number of Google accounts, each signed in from the apps by redirect (`signin`) and kept in the
//! vault as `google.<label>`. Which account a call uses is `accounts`'. The commands' params are
//! proto/google/v1/google.proto's.

pub mod accounts;
pub mod calendar;
pub mod google;
pub mod mail;
pub mod signin;

/// proto/google/v1/google.proto's messages, as buffa builds them (build.rs).
pub mod proto {
    pointman_extension::include_proto!();
}
pub use proto::google::v1;

/// The vault item with the Google OAuth client: every other `google.<label>` is an account's sign-in.
pub const CLIENT_ITEM: &str = "google.client";

/// A list in words: `a`, `a and b`, `a, b and c`.
pub fn and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `1 message`, `2 messages`.
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
