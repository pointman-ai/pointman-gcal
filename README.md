# gcal

A Pointman extension that gives every agent Google Calendar, with any number of Google accounts. Each account is
signed in from the Pointman apps and kept in the vault as `gcal.<label>`, so every machine with the extension uses the
same sign-ins, and no agent depends on one person's own connectors. Agents read freely. Creating, changing or deleting
events needs `confirm: true`, given once the person has said yes. Mail is a separate extension, gmail.

| File | What |
|---|---|
| `SKILL.md` | The agents' guidance: the commands, accounts, asking first |
| `proto/gcal/v1/gcal.proto` | The commands' params and results, and the settings |
| `src/` | The process: `signin` (authorization code + PKCE, shared or own client), `accounts` (which account a call uses), `calendar`, `google` (the HTTP calls, and refreshes through core) |

## Install

```
pointman add pointman-ai/pointman-gcal
```

## Sign accounts in

**`/gcal add <label>`** in the app for each account (`/gcal add work`, `/gcal add personal`), and tap Sign in. Google's
page opens; pick the account and allow it. Pointman finishes the sign-in by itself: nothing to paste.

On a hosted Pointman that's all: the sign-in goes through Pointman's own Google client, core's shared client, whose
redirect URI is `https://api.pointman.ai/v1/oauth/google/callback`. The extension never holds that client's secret:
it exchanges the code and refreshes the account's token through core. The vault keeps the refresh token with `shared:`
before it, so its refreshes keep going through core.

The sign-in asks for `openid email` and `calendar`. It's Google's authorization-code flow with PKCE, coming back by
redirect; Google's device flow can't grant Calendar. It needs node 0.4.12 or later.

### A self-hosted Pointman: your own OAuth client, once

A Pointman with no shared client (its core says it has none) signs in with a client of yours, before the first
`/gcal add`:

1. In [Google Cloud console](https://console.cloud.google.com/), in a project of yours:
   - APIs & Services › Library: turn on the **Google Calendar API**.
   - OAuth consent screen: add the scopes above, and publish it **In production** (or make it **Internal** for a
     Google Workspace organisation). While it's in Testing, Google's refresh tokens run out after 7 days.
   - Credentials › Create OAuth client ID › **Web application**, with the authorised redirect URI
     `https://<your Pointman>/oauth/callback`: your core's own callback, not the shared client's.

   gmail can use the same client: each sign-in asks only for its own extension's scopes.
2. **`/gcal client`** in the app: paste the client's ID, a space, then its secret.

An account is refreshed with the client that signed it in, so accounts signed in before a Pointman had a shared client
keep using `gcal.client`.

## The commands

| Command | What it does |
|---|---|
| `gcal.accounts {}` | Each account's label and address, and whether its sign-in works |
| `gcal.calendars {account?}` | The calendars each account has |
| `gcal.events {from, to, calendar?, query?, account?}` | Events between two times, soonest first, each repeat on its own |
| `gcal.create {title, start, end, …, confirm}` | A new event, with attendees, a Meet link (`meet`) and invitations (`notify`) if asked |
| `gcal.update {id, …, confirm}` | Changes only the fields given (`attendees` replaces the whole list) |
| `gcal.delete {id, confirm}` | Deletes an event |
| `gcal.freebusy {from, to, emails?, account?}` | Busy times of the account's calendar, or of the people named |
| `/gcal add`, `/gcal client` | Open the forms for an account's sign-in and for a self-hosted Pointman's OAuth client, in the app |

**Which account.** A call may name one with `account`; the `account` setting is the default. With neither, a read uses
every account signed in and says which each result came from, and a write uses the only one, or stops and lists them.

## Develop

`cargo test` runs the commands under the SDK's stand-in node against a fake Google on a local port (`tests/common`):
no real account is called. Change a command's params in the proto, then `pointman-extension manifest write` to write
extension.toml's schemas again; `tests/manifest.rs` fails while they differ. A release is a tag that's
extension.toml's version (`git tag v0.1.0 && git push origin v0.1.0`): `release.yml` builds each platform.
