# google

A Pointman extension that gives every agent Gmail and Google Calendar, with any number of Google accounts. Each
account is signed in from the Pointman apps and kept in the vault as `google.<label>`, so every machine with the
extension uses the same sign-ins, and no agent depends on one person's own connectors. Agents read freely; sending
mail and creating, changing or deleting events need `confirm: true`, given once the person has said yes.

| File | What |
|---|---|
| `SKILL.md` | The agents' guidance: the commands, accounts, asking first, keeping mail private |
| `proto/google/v1/google.proto` | The commands' params and results, and the settings |
| `src/` | The process: `signin` (authorization code + PKCE), `accounts` (which account a call uses), `mail`, `calendar`, `google` (the HTTP calls) |

## Install

```
pointman add pointman-ai/pointman-google
```

## Set it up once

1. **An OAuth client.** In [Google Cloud console](https://console.cloud.google.com/), in a project of yours:
   - APIs & Services › Library: turn on the **Gmail API** and the **Google Calendar API**.
   - OAuth consent screen: add the scopes below, and publish it **In production** (or make it **Internal** for a
     Google Workspace organisation). While it's in Testing, Google's refresh tokens run out after 7 days.
   - Credentials › Create OAuth client ID › **Web application**, with the authorised redirect URI
     `https://<your Pointman>/oauth/callback` (`https://james.pointman.network/oauth/callback`, say).
2. **`/google client`** in the app: paste the client's ID, a space, then its secret.
3. **`/google add <label>`** for each account (`/google add work`, `/google add personal`), and tap Sign in. Google's
   page opens; pick the account and allow it. Pointman finishes the sign-in by itself: nothing to paste.

The sign-in asks for `openid email`, `gmail.modify` (read, label and draft mail), `gmail.send` and `calendar`. It's
Google's authorization-code flow with PKCE, coming back through core's `/oauth/callback`; Google's device flow can't
grant Gmail or Calendar.

## The commands

| Command | What it does |
|---|---|
| `google.accounts {}` | Each account's label and address, and whether its sign-in works |
| `google.mail_search {query, account?, limit?}` | Messages matching a Gmail search, newest first, from every account unless one is named |
| `google.mail_read {id, account?}` | One message: headers, its text (`body`), attachments' names |
| `google.mail_draft {to, cc?, subject, body, reply_to?, account?}` | A draft, nothing sent; `reply_to` puts it in the message's thread |
| `google.mail_send {draft \| to, cc?, subject, body, account?, confirm}` | Sends a draft or a new message. Refused without `confirm: true` |
| `google.calendars {account?}` | The calendars each account has |
| `google.events {from, to, calendar?, query?, account?}` | Events between two times, soonest first, each repeat on its own |
| `google.event_create {title, start, end, …, confirm}` | A new event, with attendees, a Meet link (`meet`) and invitations (`notify`) if asked |
| `google.event_update {id, …, confirm}` | Changes only the fields given (`attendees` replaces the whole list) |
| `google.event_delete {id, confirm}` | Deletes an event |
| `google.freebusy {from, to, emails?, account?}` | Busy times of the account's calendar, or of the people named |
| `/google add`, `/google client` | Open the forms for an account's sign-in and for the OAuth client, in the app |

**Which account.** A call may name one with `account`; the `account` setting is the default. With neither, a read uses
every account signed in and says which each result came from, and a write uses the only one, or stops and lists them.

## Develop

`cargo test` runs the commands under the SDK's stand-in node against a fake Google on a local port (`tests/common`):
no real account is called. Change a command's params in the proto, then `pointman-extension manifest write` to write
extension.toml's schemas again; `tests/manifest.rs` fails while they differ. A release is a tag that's
extension.toml's version (`git tag v0.1.0 && git push origin v0.1.0`): `release.yml` builds each platform.
