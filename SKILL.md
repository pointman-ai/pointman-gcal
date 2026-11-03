---
name: gcal
description: How to list a person's Google Calendar events and busy times, and create, change and delete their events on Pointman, with any number of Google accounts, through the gcal extension's commands. Use it when a task touches their calendar or availability.
---

# Google Calendar

For tasks on the person's calendar: what's on this week, booking or moving a meeting, when people are free. Skip it
otherwise. Mail is the gmail extension's.

**The commands,** on any machine with the extension: `create_job` (or `/gcal <verb>` in a thread).

| For | Command |
|---|---|
| Which accounts are signed in, their addresses, whether they work | `gcal.accounts {}` |
| The calendars an account has | `gcal.calendars {account?}` |
| Events between two times, soonest first | `gcal.events {from, to, calendar?, query?, account?}` |
| A new event (Meet link with `meet`, invitations with `notify`) | `gcal.create {title, start, end, time_zone?, description?, location?, attendees?, meet?, notify?, calendar?, account?, confirm}` |
| Change an event: only the fields given (`attendees` replaces the whole list) | `gcal.update {id, title?, start?, end?, …, attendees?, notify?, account?, confirm}` |
| Delete an event | `gcal.delete {id, notify?, account?, confirm}` |
| When people are busy | `gcal.freebusy {from, to, emails?, account?}` |

**Accounts.** Each is a label (`work`, `personal`): the vault's `gcal.<label>`. Name one with `account`. Without it, a
read uses the `account` setting's, else every account, and says which each result came from; a write uses the setting's
or the only one, and with several it stops and lists them. Then ask the person which, never guess. `gcal.accounts`
says whose each one is.

**Ask first for calendar writes.** Creating, changing or deleting an event is the person's call. Post exactly what will
happen as a decision: its title, times, calendar, attendees, and whether they're emailed (`notify`). Pass
`confirm: true` only after their yes, for what they said yes to. Never set it on your own.

**`notify: true` is a send.** It emails every attendee from that account: the invitation, the change, or that the event
is off. Ask it with the event, naming who gets it, and set it only when the person says to tell them. It's off unless
given, and the event still needs `confirm`.

**Adding or removing someone:** `gcal.update`'s `attendees` replaces the whole list, so whoever you leave out is taken
off the event (and with `notify`, emailed that they're off it). Read the event first (`gcal.events` gives its
attendees), then pass everyone who should stay, plus the change.

**Calendars are private.** Events' descriptions, places and attendees are the person's: use them to do the task, and
post them only when asked.

**Times** are RFC 3339 with an offset (`2026-11-05T10:00:00+08:00`), or a local time with `time_zone`
(`2026-11-05T10:00`, `Asia/Singapore`), or a day for an all-day event (`2026-11-05`; its `end` is the day after its last).
A range's day starts at midnight UTC: give times with the person's offset for "today" or "this week".

- **Not signed in?** Ask the person to run `/gcal client` in the app once (their Google Cloud OAuth client), then
  `/gcal add <label>` for each account. Never ask for, paste or post a token, a code or the client's secret.
- An error that says a sign-in no longer works: ask them to run `/gcal add <label>` again for that account.
