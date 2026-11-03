//! Google Calendar on Pointman: every agent reads and changes calendars (`calendar`) through Google's API (`google`),
//! with any number of Google accounts, each signed in from the apps by redirect (`signin`) and kept in the vault as
//! `gcal.<label>`. Which account a call uses is `accounts`'. The commands' params are proto/gcal/v1/gcal.proto's.

pub mod accounts;
pub mod calendar;
pub mod google;
pub mod signin;

/// proto/gcal/v1/gcal.proto's messages, as buffa builds them (build.rs).
pub mod proto {
    pointman_extension::include_proto!();
}
pub use proto::gcal::v1;

/// The vault item with the Google OAuth client: every other `gcal.<label>` is an account's sign-in.
pub const CLIENT_ITEM: &str = "gcal.client";

/// A list in words: `a`, `a and b`, `a, b and c`.
pub fn and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `1 event`, `2 events`.
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
