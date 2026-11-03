//! Google Calendar's events as the commands give them, and the times they're given: RFC 3339, a local time with a
//! time zone, or a day for an all-day event.

use anyhow::bail;
use regex::Regex;
use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::{Date, Month, OffsetDateTime};

use crate::v1::{Attendee, Event};

/// A time as a command was given it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum When {
    /// A day, 2026-11-05: an all-day event's, or a range's from its midnight UTC.
    Day(String),
    /// RFC 3339 with its offset, seconds added if they were left out.
    At(String),
    /// A local time with no offset, 2026-11-05T10:00:00: only with a time zone.
    Local(String),
}

fn day(s: &str) -> Option<Date> {
    let re = Regex::new(r"^(\d{4})-(\d{2})-(\d{2})$").expect("compiles");
    let c = re.captures(s)?;
    let month = Month::try_from(c[2].parse::<u8>().ok()?).ok()?;
    Date::from_calendar_date(c[1].parse().ok()?, month, c[3].parse().ok()?).ok()
}

/// A time as one of the forms a command takes, or why it isn't one.
pub fn when(what: &str, given: &str) -> anyhow::Result<When> {
    let s = given.trim();
    if day(s).is_some() {
        return Ok(When::Day(s.to_string()));
    }
    // seconds left out (10:00+08:00): Google and RFC 3339 want them
    let short = Regex::new(r"^(\d{4}-\d{2}-\d{2}[Tt ]\d{2}:\d{2})([Zz]|[+-]\d{2}:\d{2})?$").expect("compiles");
    let s = match short.captures(s) {
        Some(c) => format!("{}:00{}", &c[1], c.get(2).map(|m| m.as_str()).unwrap_or("")),
        None => s.to_string(),
    };
    let s = s.replacen(' ', "T", 1);
    if OffsetDateTime::parse(&s, &Rfc3339).is_ok() {
        return Ok(When::At(s));
    }
    let local = Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?$").expect("compiles");
    if local.is_match(&s) && day(&s[..10]).is_some() {
        return Ok(When::Local(s));
    }
    bail!("{what}: {given:?} isn't a time: give RFC 3339 (2026-11-05T10:00:00+08:00), or a day (2026-11-05)")
}

/// A range's end or start, as Calendar's timeMin and timeMax take it: RFC 3339, a day from its midnight UTC.
pub fn bound(what: &str, given: &str) -> anyhow::Result<String> {
    match when(what, given)? {
        When::Day(d) => Ok(format!("{d}T00:00:00Z")),
        When::At(s) => Ok(s),
        When::Local(_) => bail!("{what}: {given:?} has no offset: add one (+08:00, or Z for UTC)"),
    }
}

/// A range's two ends, checked the right way round.
pub fn range(from: &str, to: &str) -> anyhow::Result<(String, String)> {
    let (a, b) = (bound("from", from)?, bound("to", to)?);
    if let (Ok(x), Ok(y)) = (OffsetDateTime::parse(&a, &Rfc3339), OffsetDateTime::parse(&b, &Rfc3339)) {
        if y <= x {
            bail!("to ({to}) isn't after from ({from})");
        }
    }
    Ok((a, b))
}

/// An event's start or end as Calendar takes it: `{date}` for a day, `{dateTime, timeZone?}` for a time.
pub fn event_time(what: &str, w: &When, zone: &str) -> anyhow::Result<Value> {
    Ok(match w {
        When::Day(d) => json!({"date": d}),
        When::At(s) if zone.is_empty() => json!({"dateTime": s}),
        When::At(s) => json!({"dateTime": s, "timeZone": zone}),
        When::Local(s) if zone.is_empty() => {
            bail!("{what}: {s} has no offset: add one (+08:00), or give time_zone (Asia/Singapore)")
        }
        When::Local(s) => json!({"dateTime": s, "timeZone": zone}),
    })
}

/// An event's start and end, both days or both times, the end after the start.
pub fn times(start: &str, end: &str, zone: &str) -> anyhow::Result<(Value, Value)> {
    let (a, b) = (when("start", start)?, when("end", end)?);
    match (&a, &b) {
        (When::Day(_), When::Day(_)) => {}
        (When::Day(_), _) | (_, When::Day(_)) => {
            bail!("start and end are both days (an all-day event) or both times, not one of each")
        }
        _ => {}
    }
    let later = match (&a, &b) {
        (When::Day(x), When::Day(y)) => day(y) > day(x),
        (When::At(x), When::At(y)) => match (OffsetDateTime::parse(x, &Rfc3339), OffsetDateTime::parse(y, &Rfc3339)) {
            (Ok(x), Ok(y)) => y > x,
            _ => true,
        },
        (When::Local(x), When::Local(y)) => y > x,
        _ => true,
    };
    if !later {
        let hint = if matches!(a, When::Day(_)) { " (an all-day event's end is the day after its last)" } else { "" };
        bail!("end ({end}) isn't after start ({start}){hint}");
    }
    Ok((event_time("start", &a, zone)?, event_time("end", &b, zone)?))
}

/// When a start is, in seconds, to put events from several accounts in order: a day from its midnight UTC.
pub fn instant(start: &str) -> i64 {
    if let Ok(t) = OffsetDateTime::parse(start, &Rfc3339) {
        return t.unix_timestamp();
    }
    day(start).map(|d| d.midnight().assume_utc().unix_timestamp()).unwrap_or(0)
}

/// An event's start or end as Google gives it: its time, or its day for an all-day event.
fn time_of(v: &Value) -> (String, bool) {
    match (v["dateTime"].as_str(), v["date"].as_str()) {
        (Some(t), _) => (t.to_string(), false),
        (None, Some(d)) => (d.to_string(), true),
        _ => (String::new(), false),
    }
}

/// An event as the commands give it.
pub fn event(account: &str, calendar: &str, e: &Value) -> Event {
    let text = |k: &str| e[k].as_str().unwrap_or_default().to_string();
    let (start, all_day) = time_of(&e["start"]);
    let (end, _) = time_of(&e["end"]);
    let meet = e["hangoutLink"].as_str().map(String::from).unwrap_or_else(|| {
        e["conferenceData"]["entryPoints"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["entryPointType"] == "video")
            .and_then(|p| p["uri"].as_str())
            .unwrap_or_default()
            .to_string()
    });
    Event {
        account: account.to_string(),
        calendar: calendar.to_string(),
        id: text("id"),
        title: text("summary"),
        start,
        end,
        all_day,
        location: text("location"),
        description: text("description"),
        attendees: e["attendees"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| Attendee {
                email: a["email"].as_str().unwrap_or_default().to_string(),
                name: a["displayName"].as_str().unwrap_or_default().to_string(),
                response: a["responseStatus"].as_str().unwrap_or_default().to_string(),
                organizer: a["organizer"].as_bool().unwrap_or(false),
                ..Default::default()
            })
            .collect(),
        organizer: e["organizer"]["email"].as_str().unwrap_or_default().to_string(),
        status: text("status"),
        url: text("htmlLink"),
        meet,
        recurring_event_id: text("recurringEventId"),
        ..Default::default()
    }
}

/// An event in a line: when, what, where.
pub fn line(e: &Event) -> String {
    let when = if e.all_day { format!("{} (all day)", e.start) } else { format!("{} to {}", e.start, e.end) };
    let title = if e.title.is_empty() { "(no title)" } else { e.title.as_str() };
    let place = if e.location.is_empty() { String::new() } else { format!(", at {}", e.location) };
    format!("{when}: {title}{place} [{}]", e.id)
}

/// Attendees' addresses checked: each has an @, and none has a line break.
pub fn check_addresses(what: &str, list: &[String]) -> anyhow::Result<()> {
    for a in list {
        if a.contains(['\r', '\n']) {
            bail!("{what}: {a:?} has a line break");
        }
        if !a.contains('@') {
            bail!("{what}: {a:?} isn't an address (name@example.com)");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_come_as_rfc_3339_local_or_a_day() {
        assert_eq!(when("start", "2026-11-05").unwrap(), When::Day("2026-11-05".into()));
        assert_eq!(when("start", "2026-11-05T10:00:00+08:00").unwrap(), When::At("2026-11-05T10:00:00+08:00".into()));
        assert_eq!(when("start", "2026-11-05T10:00Z").unwrap(), When::At("2026-11-05T10:00:00Z".into()));
        assert_eq!(when("start", "2026-11-05 10:00+08:00").unwrap(), When::At("2026-11-05T10:00:00+08:00".into()));
        assert_eq!(when("start", "2026-11-05T10:00").unwrap(), When::Local("2026-11-05T10:00:00".into()));
        assert!(when("start", "next tuesday").is_err());
        assert!(when("start", "2026-13-05").is_err());
    }

    #[test]
    fn a_range_is_rfc_3339_and_the_right_way_round() {
        assert_eq!(
            range("2026-11-03", "2026-11-10T00:00:00+08:00").unwrap(),
            ("2026-11-03T00:00:00Z".into(), "2026-11-10T00:00:00+08:00".into())
        );
        assert!(range("2026-11-10", "2026-11-03").unwrap_err().to_string().contains("isn't after"));
        assert!(range("2026-11-03T10:00:00", "2026-11-04").unwrap_err().to_string().contains("no offset"));
    }

    #[test]
    fn an_events_times_are_both_days_or_both_times() {
        let (s, e) = times("2026-11-05", "2026-11-06", "").unwrap();
        assert_eq!((s, e), (json!({"date": "2026-11-05"}), json!({"date": "2026-11-06"})));
        let (s, _) = times("2026-11-05T10:00", "2026-11-05T11:00", "Asia/Singapore").unwrap();
        assert_eq!(s, json!({"dateTime": "2026-11-05T10:00:00", "timeZone": "Asia/Singapore"}));
        assert!(times("2026-11-05T10:00", "2026-11-05T11:00", "").unwrap_err().to_string().contains("time_zone"));
        assert!(times("2026-11-05", "2026-11-05T11:00:00Z", "").is_err());
        let same_day = times("2026-11-05", "2026-11-05", "").unwrap_err().to_string();
        assert!(same_day.contains("the day after"), "{same_day}");
        assert!(times("2026-11-05T11:00:00Z", "2026-11-05T10:00:00Z", "").is_err());
    }

    #[test]
    fn an_event_reads_as_google_gives_it() {
        let e = event(
            "work",
            "primary",
            &json!({"id": "ev1", "summary": "Standup", "status": "confirmed", "htmlLink": "https://calendar.google.com/x",
                "start": {"dateTime": "2026-11-05T10:00:00+08:00"}, "end": {"dateTime": "2026-11-05T10:15:00+08:00"},
                "hangoutLink": "https://meet.google.com/abc-defg-hij",
                "attendees": [{"email": "bob@example.com", "responseStatus": "accepted"}],
                "organizer": {"email": "james@example.com"}}),
        );
        assert_eq!((e.title.as_str(), e.start.as_str(), e.all_day), ("Standup", "2026-11-05T10:00:00+08:00", false));
        assert_eq!(e.meet, "https://meet.google.com/abc-defg-hij");
        assert_eq!(e.attendees[0].response, "accepted");
        assert_eq!(line(&e), "2026-11-05T10:00:00+08:00 to 2026-11-05T10:15:00+08:00: Standup [ev1]");
        let all_day = event(
            "work",
            "primary",
            &json!({"id": "ev2", "start": {"date": "2026-11-06"}, "end": {"date": "2026-11-07"}}),
        );
        assert!(all_day.all_day);
        assert_eq!(line(&all_day), "2026-11-06 (all day): (no title) [ev2]");
    }
}
