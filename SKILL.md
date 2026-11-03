---
name: google
description: How to read and search a person's Gmail, draft and send mail, and list, create, change and delete their Google Calendar events on Pointman, with any number of Google accounts, through the google extension's commands. Use it when a task touches their mail, calendar or availability.
---

# Gmail and Google Calendar

For tasks on the person's mail or calendar: finding a message, answering one, what's on this week, booking or moving
a meeting, when people are free. Skip it otherwise.

**The commands,** on any machine with the extension: `create_job` (or `/google <verb>` in a thread).

| For | Command |
|---|---|
| Which accounts are signed in, their addresses, whether they work | `google.accounts {}` |
| Find mail (Gmail's search: `from:bob newer_than:7d`, `is:unread in:inbox`) | `google.mail_search {query, account?, limit?}` |
| Read one message: headers, its text in `body`, attachments' names | `google.mail_read {id, account?}` |
| A draft, nothing sent (`reply_to` a message's id: in its thread, to its sender, `Re:` its subject) | `google.mail_draft {to, cc?, subject, body, reply_to?, account?}` |
| Send a draft, or a new message | `google.mail_send {draft \| to, cc?, subject, body, account?, confirm}` |
| The calendars an account has | `google.calendars {account?}` |
| Events between two times, soonest first | `google.events {from, to, calendar?, query?, account?}` |
| A new event (Meet link with `meet`, invitations with `notify`) | `google.event_create {title, start, end, time_zone?, description?, location?, attendees?, meet?, notify?, calendar?, account?, confirm}` |
| Change an event: only the fields given | `google.event_update {id, title?, start?, end?, …, notify?, account?, confirm}` |
| Delete an event | `google.event_delete {id, notify?, account?, confirm}` |
| When people are busy | `google.freebusy {from, to, emails?, account?}` |

**Accounts.** Each is a label (`work`, `personal`): the vault's `google.<label>`. Name one with `account`. Without it, a
read uses the `account` setting's, else every account, and says which each result came from; a write uses the setting's
or the only one, and with several it stops and lists them. Then ask the person which, never guess. `google.accounts`
says whose each one is.

**Ask first, every time.** Sending mail, and creating, changing or deleting an event, are the person's call. Post
exactly what will happen as a decision: for mail, who it's to and from which account, the subject and the whole text;
for an event, its title, times, calendar, attendees, and whether they're emailed (`notify`). Pass `confirm: true` only
after their yes, for what they said yes to. Never set it on your own. A draft needs no yes, but say what you drafted.

**`notify: true` is a send.** It emails every attendee from that account: the invitation, the change, or that the event
is off. Ask it as you would a message, naming who gets it, and set it only when the person says to tell them. It's off
unless given, and the event still needs `confirm`.

**Mail is private.** A message's words come back in `body`, and a search's in `snippet`: read them to do the task, then
tell the person what's there in your own words. Post a message's text, or forward it, only when they ask. Run mail
commands without `thread`, so their results come back to you rather than into a thread.

**Times** are RFC 3339 with an offset (`2026-11-05T10:00:00+08:00`), or a local time with `time_zone`
(`2026-11-05T10:00`, `Asia/Singapore`), or a day for an all-day event (`2026-11-05`; its `end` is the day after its last).
A range's day starts at midnight UTC: give times with the person's offset for "today" or "this week".

- **Not signed in?** Ask the person to run `/google client` in the app once (their Google Cloud OAuth client), then
  `/google add <label>` for each account. Never ask for, paste or post a token, a code or the client's secret.
- An error that says a sign-in no longer works: ask them to run `/google add <label>` again for that account.
