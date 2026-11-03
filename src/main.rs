//! The extension's process: the node starts it (`bin/google`, from the extension's folder) and talks to it over stdin
//! and stdout. It searches, reads, drafts and sends mail, lists and changes calendar events, and signs Google accounts
//! in from the apps. Nothing reaches Google without a sign-in in the vault; nothing is sent, and no event made, changed
//! or deleted, without `confirm`; and no token, code or message body is ever logged.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use google::accounts::{self, item};
use google::google::{access_token, agent, segment, Api, Endpoints, OAuthClient};
use google::mail::{self, Draft};
use google::signin::{self, SignInRequest, SignIns};
use google::v1::{
    Account, AccountsParams, AccountsResult, AddParams, AddResult, Busy, Calendar, CalendarsParams, CalendarsResult,
    ClientParams, ClientResult, EventCreateParams, EventCreateResult, EventDeleteParams, EventDeleteResult,
    EventUpdateParams, EventUpdateResult, EventsParams, EventsResult, FreebusyParams, FreebusyResult, MailDraftParams,
    MailDraftResult, MailReadParams, MailReadResult, MailSearchParams, MailSearchResult, MailSendParams,
    MailSendResult, Problem, Settings, Span,
};
use google::{and, calendar, count, CLIENT_ITEM};
use pointman_extension::protocol::{CredentialsCodeRequest, CredentialsCodeResponse};
use pointman_extension::{Extension, Handle, Job};
use regex::Regex;
use serde_json::{json, Value};

struct Ext {
    h: Handle,
    agent: ureq::Agent,
    ends: Endpoints,
    /// Access tokens by vault item, until they run out.
    tokens: Mutex<HashMap<String, (String, Instant)>>,
}

impl Ext {
    /// The accounts signed in, by label.
    fn have(&self) -> anyhow::Result<Vec<String>> {
        Ok(accounts::labels(self.h.secrets("google.")?))
    }

    fn setting(&self) -> String {
        self.h.typed_settings::<Settings>().map(|s| s.account).unwrap_or_default()
    }

    fn reads(&self, asked: &str) -> anyhow::Result<Vec<String>> {
        accounts::for_read(&self.have()?, asked, &self.setting())
    }

    fn write(&self, asked: &str, kind: &str) -> anyhow::Result<String> {
        accounts::for_write(&self.have()?, asked, &self.setting(), kind)
    }

    /// The Web application client the sign-ins go through: the vault's `google.client`.
    fn client(&self) -> anyhow::Result<OAuthClient> {
        let value = self.h.secret(CLIENT_ITEM).map_err(|_| {
            anyhow!(
                "no Google OAuth client yet: type /google client in the app and paste a Web application client's ID \
                 and secret"
            )
        })?;
        OAuthClient::parse(&value).ok_or_else(|| {
            anyhow!(
                "{CLIENT_ITEM} isn't a client ID and secret (…apps.googleusercontent.com, a space, then the secret): \
                 add it again with /google client"
            )
        })
    }

    /// An access token for an account, made again when it's within a minute of running out, or `fresh`.
    fn token(&self, label: &str, fresh: bool) -> anyhow::Result<String> {
        let name = item(label);
        if !fresh {
            let cached = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((token, until)) = cached.get(&name) {
                if *until > Instant::now() + Duration::from_secs(60) {
                    return Ok(token.clone());
                }
            }
        }
        let value = self.h.secret(&name)?;
        let refresh = Regex::new(r"1//[A-Za-z0-9_-]+")
            .expect("compiles")
            .find(&value)
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| anyhow!("{name} isn't a Google refresh token: sign it in again with /google add {label}"))?;
        let (token, lasts) = access_token(&self.agent, &self.ends, &self.client()?, &refresh, &name)?;
        self.tokens.lock().unwrap_or_else(|e| e.into_inner()).insert(name, (token.clone(), Instant::now() + lasts));
        Ok(token)
    }

    fn api<T>(&self, label: &str, f: impl FnOnce(&Api) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let token = |fresh: bool| self.token(label, fresh);
        f(&Api { agent: &self.agent, ends: &self.ends, token: &token })
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn problem(account: &str, e: &anyhow::Error) -> Problem {
    Problem { account: account.to_string(), error: format!("{e:#}"), ..Default::default() }
}

/// Each account's failure in a line, for a read's text.
fn failures(failed: &[Problem]) -> String {
    failed.iter().map(|f| format!("\nCouldn't ask {}: {}", f.account, f.error)).collect()
}

/// A read that asked every account it was given, and every one failed: the error is theirs.
fn all_failed(labels: &[String], failed: &[Problem]) -> anyhow::Result<()> {
    if !labels.is_empty() && failed.len() == labels.len() {
        let each: Vec<String> = failed.iter().map(|f| format!("{}: {}", f.account, f.error)).collect();
        bail!("{}", each.join("; "));
    }
    Ok(())
}

fn message_url(api: &Api, id: &str) -> String {
    api.gmail(&format!("users/me/messages/{}", segment(id)))
}

fn calendar_id(given: &str) -> String {
    match given.trim() {
        "" => "primary".to_string(),
        c => c.to_string(),
    }
}

fn accounts_list(x: &Ext, _: &Job, _: AccountsParams) -> anyhow::Result<AccountsResult> {
    let have = x.have()?;
    if have.is_empty() {
        return Ok(AccountsResult {
            text: "No Google account is signed in: add the OAuth client with /google client if it isn't there, then \
                   /google add <label> in the app."
                .into(),
            ..Default::default()
        });
    }
    let setting = x.setting();
    let mut out = AccountsResult::default();
    let mut lines = Vec::new();
    for label in &have {
        let mut a = Account { label: label.clone(), chosen: *label == setting, ..Default::default() };
        match x.api(label, |api| api.get(&api.gmail("users/me/profile"), &[])) {
            Ok(v) => {
                a.email = s(&v["emailAddress"]);
                a.works = true;
                lines.push(format!("{label} ({})", a.email));
            }
            Err(e) => {
                a.error = format!("{e:#}");
                lines.push(format!("{label} (not working: {})", a.error));
            }
        }
        out.accounts.push(a);
    }
    let chosen = if setting.is_empty() { String::new() } else { format!(" The account setting uses {setting}.") };
    out.text = format!("{}: {}.{chosen}", count(have.len(), "Google account", "Google accounts"), lines.join(", "));
    Ok(out)
}

fn mail_search(x: &Ext, _: &Job, p: MailSearchParams) -> anyhow::Result<MailSearchResult> {
    let labels = x.reads(&p.account)?;
    let limit = (if p.limit <= 0 { 10 } else { p.limit.min(50) }).to_string();
    let mut out = MailSearchResult { accounts: labels.clone(), ..Default::default() };
    for label in &labels {
        let got = x.api(label, |api| {
            let list =
                api.get(&api.gmail("users/me/messages"), &[("q", p.query.as_str()), ("maxResults", limit.as_str())])?;
            let mut found = Vec::new();
            for m in list["messages"].as_array().into_iter().flatten() {
                let v = api.get(
                    &message_url(api, m["id"].as_str().unwrap_or_default()),
                    &[
                        ("format", "metadata"),
                        ("metadataHeaders", "From"),
                        ("metadataHeaders", "To"),
                        ("metadataHeaders", "Subject"),
                        ("metadataHeaders", "Date"),
                    ],
                )?;
                found.push(mail::summary(label, &v));
            }
            Ok(found)
        });
        match got {
            Ok(found) => out.messages.extend(found),
            Err(e) => out.failed.push(problem(label, &e)),
        }
    }
    all_failed(&labels, &out.failed)?;
    out.messages.sort_by(|a, b| b.date.cmp(&a.date));
    let several = labels.len() > 1;
    let mut text = if out.messages.is_empty() {
        format!("No messages for \"{}\" in {}.", p.query, and(&labels))
    } else {
        format!(
            "{} for \"{}\" in {}, newest first:",
            count(out.messages.len(), "message", "messages"),
            p.query,
            and(&labels)
        )
    };
    for m in &out.messages {
        let unread = if m.unread { " (unread)" } else { "" };
        let at = if several { format!("{} ", m.account) } else { String::new() };
        text.push_str(&format!("\n- {} {}: {}{unread} [{at}{}]", m.date, m.from, m.subject, m.id));
    }
    text.push_str(&failures(&out.failed));
    out.text = text;
    Ok(out)
}

fn mail_read(x: &Ext, _: &Job, p: MailReadParams) -> anyhow::Result<MailReadResult> {
    let labels = x.reads(&p.account)?;
    let mut errors = Vec::new();
    for label in &labels {
        match x.api(label, |api| api.get(&message_url(api, &p.id), &[("format", "full")])) {
            Ok(v) => return Ok(read(label, &v)),
            Err(e) => errors.push(format!("{label}: {e:#}")),
        }
    }
    bail!("no message {} in {} ({})", p.id, and(&labels), errors.join("; "))
}

fn read(label: &str, v: &Value) -> MailReadResult {
    let h = mail::headers(v);
    let get = |k: &str| h.get(k).cloned().unwrap_or_default();
    let (body, from_html) = mail::text(v);
    let (body, truncated) = mail::cut(body, mail::MAX_BODY);
    let attachments = mail::attachments(v);
    let mut out = MailReadResult {
        account: label.to_string(),
        id: s(&v["id"]),
        thread_id: s(&v["threadId"]),
        from: get("from"),
        to: get("to"),
        cc: get("cc"),
        subject: get("subject"),
        date: mail::date(v),
        labels: mail::labels(v),
        from_html,
        truncated,
        message_id: get("message-id"),
        ..Default::default()
    };
    let size = format!(
        "{} of text{}{}",
        count(body.chars().count(), "character", "characters"),
        if from_html { ", from its HTML" } else { "" },
        if truncated { ", cut at 100,000" } else { "" }
    );
    let files = if attachments.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = attachments.iter().map(|a| a.name.clone()).collect();
        format!("; {}: {}", count(attachments.len(), "attachment", "attachments"), names.join(", "))
    };
    out.text = format!(
        "\"{}\" from {}, {}, in {label}: {size}{files}. Its words are in body: private, so say what it's about and \
         post them only when asked.",
        out.subject, out.from, out.date
    );
    out.body = body;
    out.attachments = attachments;
    out
}

fn mail_draft(x: &Ext, _: &Job, p: MailDraftParams) -> anyhow::Result<MailDraftResult> {
    let mut d = Draft {
        to: p.to.clone(),
        cc: p.cc.clone(),
        subject: p.subject.clone(),
        body: p.body.clone(),
        ..Default::default()
    };
    let reply = p.reply_to.trim();
    if reply.is_empty() {
        if d.subject.trim().is_empty() {
            bail!("subject: what is it about? (only a reply takes its message's)");
        }
        d.rfc5322()?;
    } else {
        mail::check_addresses("to", &d.to)?;
        mail::check_addresses("cc", &d.cc)?;
    }
    let label = x.write(&p.account, "google.mail_draft")?;
    x.api(&label, |api| {
        let mut thread = String::new();
        if !reply.is_empty() {
            let orig = api.get(
                &message_url(api, reply),
                &[
                    ("format", "metadata"),
                    ("metadataHeaders", "Message-ID"),
                    ("metadataHeaders", "References"),
                    ("metadataHeaders", "Subject"),
                    ("metadataHeaders", "From"),
                    ("metadataHeaders", "Reply-To"),
                ],
            )?;
            let h = mail::headers(&orig);
            thread = s(&orig["threadId"]);
            d.in_reply_to = h.get("message-id").cloned().unwrap_or_default();
            d.references = h.get("references").cloned().unwrap_or_default();
            if d.to.is_empty() {
                let back = h.get("reply-to").or_else(|| h.get("from")).cloned().unwrap_or_default();
                if !back.is_empty() {
                    d.to = vec![back];
                }
            }
            if d.subject.trim().is_empty() {
                d.subject = mail::re(&h.get("subject").cloned().unwrap_or_default());
            }
        }
        let raw = d.raw()?;
        let mut message = json!({ "raw": raw });
        if !thread.is_empty() {
            message["threadId"] = json!(thread);
        }
        let made = api.send_json("POST", &api.gmail("users/me/drafts"), &[], &json!({"message": message}))?;
        let draft = s(&made["id"]);
        let reply_note = if thread.is_empty() { "" } else { ", in the thread it answers" };
        Ok(MailDraftResult {
            text: format!(
                "Drafted \"{}\" to {} in {label}{reply_note}: draft {draft}. Nothing is sent: google.mail_send with \
                 draft={draft} sends it once the person says yes.",
                d.subject,
                and(&d.to)
            ),
            account: label.clone(),
            draft,
            message: s(&made["message"]["id"]),
            thread_id: s(&made["message"]["threadId"]),
            to: d.to.clone(),
            subject: d.subject.clone(),
            ..Default::default()
        })
    })
}

fn mail_send(x: &Ext, _: &Job, p: MailSendParams) -> anyhow::Result<MailSendResult> {
    let draft = p.draft.trim().to_string();
    let direct = Draft {
        to: p.to.clone(),
        cc: p.cc.clone(),
        subject: p.subject.clone(),
        body: p.body.clone(),
        ..Default::default()
    };
    if draft.is_empty() {
        if p.to.is_empty() || p.subject.trim().is_empty() || p.body.trim().is_empty() {
            bail!("google.mail_send needs a draft (google.mail_draft's), or to, subject and body");
        }
        direct.rfc5322()?;
    } else if !p.to.is_empty() || !p.cc.is_empty() || !p.subject.is_empty() || !p.body.is_empty() {
        bail!("give a draft, or to, subject and body, not both: a draft is sent as it is");
    }
    let label = x.write(&p.account, "google.mail_send")?;
    if !p.confirm {
        let what =
            if draft.is_empty() { format!("\"{}\" to {}", p.subject, and(&p.to)) } else { format!("draft {draft}") };
        bail!(
            "not sent: google.mail_send needs confirm=true, given only once the person has said yes to sending {what} \
             from {label}"
        );
    }
    x.api(&label, |api| {
        let (sent, to, subject) = if draft.is_empty() {
            let raw = direct.raw()?;
            let sent = api.send_json("POST", &api.gmail("users/me/messages/send"), &[], &json!({ "raw": raw }))?;
            (sent, p.to.clone(), p.subject.clone())
        } else {
            let d = api.get(&api.gmail(&format!("users/me/drafts/{}", segment(&draft))), &[("format", "metadata")])?;
            let h = mail::headers(&d["message"]);
            let to: Vec<String> = h.get("to").map(|t| vec![t.clone()]).unwrap_or_default();
            let subject = h.get("subject").cloned().unwrap_or_default();
            let sent = api.send_json("POST", &api.gmail("users/me/drafts/send"), &[], &json!({"id": draft}))?;
            (sent, to, subject)
        };
        Ok(MailSendResult {
            text: format!("Sent \"{subject}\" to {} from {label}.", and(&to)),
            account: label.clone(),
            message: s(&sent["id"]),
            thread_id: s(&sent["threadId"]),
            to,
            subject,
            ..Default::default()
        })
    })
}

fn calendars(x: &Ext, _: &Job, p: CalendarsParams) -> anyhow::Result<CalendarsResult> {
    let labels = x.reads(&p.account)?;
    let mut out = CalendarsResult::default();
    for label in &labels {
        let got = x.api(label, |api| api.get(&api.calendar("users/me/calendarList"), &[("maxResults", "250")]));
        match got {
            Ok(v) => {
                for c in v["items"].as_array().into_iter().flatten() {
                    let name = c["summaryOverride"].as_str().or(c["summary"].as_str()).unwrap_or_default();
                    out.calendars.push(Calendar {
                        account: label.clone(),
                        id: s(&c["id"]),
                        name: name.to_string(),
                        primary: c["primary"].as_bool().unwrap_or(false),
                        role: s(&c["accessRole"]),
                        time_zone: s(&c["timeZone"]),
                        ..Default::default()
                    });
                }
            }
            Err(e) => out.failed.push(problem(label, &e)),
        }
    }
    all_failed(&labels, &out.failed)?;
    let mut text = format!("{} in {}:", count(out.calendars.len(), "calendar", "calendars"), and(&labels));
    for c in &out.calendars {
        let primary = if c.primary { ", primary" } else { "" };
        text.push_str(&format!("\n- {} ({}{primary}, {}, {}) [{}]", c.name, c.account, c.role, c.time_zone, c.id));
    }
    text.push_str(&failures(&out.failed));
    out.text = text;
    Ok(out)
}

fn events(x: &Ext, _: &Job, p: EventsParams) -> anyhow::Result<EventsResult> {
    let (from, to) = calendar::range(&p.from, &p.to)?;
    let cal = calendar_id(&p.calendar);
    let labels = x.reads(&p.account)?;
    let mut out = EventsResult::default();
    for label in &labels {
        let got = x.api(label, |api| {
            let url = api.calendar(&format!("calendars/{}/events", segment(&cal)));
            let mut found = Vec::new();
            let mut page = String::new();
            // at most 4 pages of 250
            for _ in 0..4 {
                let mut query: Vec<(&str, &str)> = vec![
                    ("timeMin", from.as_str()),
                    ("timeMax", to.as_str()),
                    ("singleEvents", "true"),
                    ("orderBy", "startTime"),
                    ("maxResults", "250"),
                ];
                if !p.query.trim().is_empty() {
                    query.push(("q", p.query.trim()));
                }
                if !page.is_empty() {
                    query.push(("pageToken", page.as_str()));
                }
                let v = api.get(&url, &query)?;
                found.extend(v["items"].as_array().into_iter().flatten().map(|e| calendar::event(label, &cal, e)));
                page = s(&v["nextPageToken"]);
                if page.is_empty() {
                    break;
                }
            }
            Ok(found)
        });
        match got {
            Ok(found) => out.events.extend(found),
            Err(e) => out.failed.push(problem(label, &e)),
        }
    }
    all_failed(&labels, &out.failed)?;
    out.events.sort_by_key(|e| calendar::instant(&e.start));
    let several = labels.len() > 1;
    let mut text = if out.events.is_empty() {
        format!("No events in {} from {} to {}.", and(&labels), p.from, p.to)
    } else {
        format!("{} in {} from {} to {}:", count(out.events.len(), "event", "events"), and(&labels), p.from, p.to)
    };
    for e in &out.events {
        let at = if several { format!("{}: ", e.account) } else { String::new() };
        text.push_str(&format!("\n- {at}{}", calendar::line(e)));
    }
    text.push_str(&failures(&out.failed));
    out.text = text;
    Ok(out)
}

fn updates(notify: bool) -> &'static str {
    if notify {
        "all"
    } else {
        "none"
    }
}

fn event_create(x: &Ext, _: &Job, p: EventCreateParams) -> anyhow::Result<EventCreateResult> {
    let zone = p.time_zone.trim();
    let (start, end) = calendar::times(&p.start, &p.end, zone)?;
    mail::check_addresses("attendees", &p.attendees)?;
    let cal = calendar_id(&p.calendar);
    let label = x.write(&p.account, "google.event_create")?;
    if !p.confirm {
        let invites = if p.attendees.is_empty() { String::new() } else { format!(", inviting {}", and(&p.attendees)) };
        bail!(
            "not made: google.event_create needs confirm=true, given only once the person has said yes to \"{}\" from \
             {} to {} on {label}'s {cal} calendar{invites}",
            p.title,
            p.start,
            p.end
        );
    }
    let mut body = json!({"summary": p.title, "start": start, "end": end});
    if !p.description.is_empty() {
        body["description"] = json!(p.description);
    }
    if !p.location.is_empty() {
        body["location"] = json!(p.location);
    }
    if !p.attendees.is_empty() {
        body["attendees"] = json!(p.attendees.iter().map(|a| json!({"email": a})).collect::<Vec<_>>());
    }
    let mut query = vec![("sendUpdates", updates(p.notify))];
    if p.meet {
        body["conferenceData"] = json!({"createRequest": {
            "requestId": signin::random(12)?,
            "conferenceSolutionKey": {"type": "hangoutsMeet"}
        }});
        query.push(("conferenceDataVersion", "1"));
    }
    x.api(&label, |api| {
        let url = api.calendar(&format!("calendars/{}/events", segment(&cal)));
        let made = api.send_json("POST", &url, &query, &body)?;
        let e = calendar::event(&label, &cal, &made);
        let meet = if e.meet.is_empty() { String::new() } else { format!(", Meet {}", e.meet) };
        let sent = if p.notify && !p.attendees.is_empty() { " Invitations sent." } else { "" };
        Ok(EventCreateResult {
            text: format!(
                "Made \"{}\" on {label}'s {cal} calendar: {}{meet} ({}).{sent}",
                e.title,
                calendar::line(&e),
                e.url
            ),
            event: e.into(),
            ..Default::default()
        })
    })
}

fn event_update(x: &Ext, _: &Job, p: EventUpdateParams) -> anyhow::Result<EventUpdateResult> {
    let zone = p.time_zone.trim();
    let mut body = json!({});
    let mut changed: Vec<String> = Vec::new();
    if let Some(t) = &p.title {
        body["summary"] = json!(t);
        changed.push("title".into());
    }
    match (&p.start, &p.end) {
        (Some(a), Some(b)) => {
            let (start, end) = calendar::times(a, b, zone)?;
            body["start"] = start;
            body["end"] = end;
            changed.extend(["start".to_string(), "end".to_string()]);
        }
        (None, None) if !zone.is_empty() => bail!("time_zone goes with a new start and end"),
        (None, None) => {}
        _ => bail!("give start and end together, so the event's length is what you mean"),
    }
    if let Some(d) = &p.description {
        body["description"] = json!(d);
        changed.push("description".into());
    }
    if let Some(l) = &p.location {
        body["location"] = json!(l);
        changed.push("location".into());
    }
    if !p.attendees.is_empty() {
        mail::check_addresses("attendees", &p.attendees)?;
        body["attendees"] = json!(p.attendees.iter().map(|a| json!({"email": a})).collect::<Vec<_>>());
        changed.push("attendees".into());
    }
    if changed.is_empty() {
        bail!("nothing to change: give title, start and end, description, location or attendees");
    }
    let cal = calendar_id(&p.calendar);
    let label = x.write(&p.account, "google.event_update")?;
    if !p.confirm {
        bail!(
            "not changed: google.event_update needs confirm=true, given only once the person has said yes to changing \
             the {} of event {} on {label}'s {cal} calendar",
            and(&changed),
            p.id
        );
    }
    x.api(&label, |api| {
        let url = api.calendar(&format!("calendars/{}/events/{}", segment(&cal), segment(&p.id)));
        let after = api.send_json("PATCH", &url, &[("sendUpdates", updates(p.notify))], &body)?;
        let e = calendar::event(&label, &cal, &after);
        Ok(EventUpdateResult {
            text: format!("Changed the {} of \"{}\": {}.", and(&changed), e.title, calendar::line(&e)),
            event: e.into(),
            changed: changed.clone(),
            ..Default::default()
        })
    })
}

fn event_delete(x: &Ext, _: &Job, p: EventDeleteParams) -> anyhow::Result<EventDeleteResult> {
    let cal = calendar_id(&p.calendar);
    let label = x.write(&p.account, "google.event_delete")?;
    if !p.confirm {
        bail!(
            "not deleted: google.event_delete needs confirm=true, given only once the person has said yes to deleting \
             event {} from {label}'s {cal} calendar",
            p.id
        );
    }
    x.api(&label, |api| {
        let url = api.calendar(&format!("calendars/{}/events/{}", segment(&cal), segment(&p.id)));
        let e = calendar::event(&label, &cal, &api.get(&url, &[])?);
        api.delete(&url, &[("sendUpdates", updates(p.notify))])?;
        Ok(EventDeleteResult {
            text: format!("Deleted \"{}\" ({}) from {label}'s {cal} calendar.", e.title, calendar::line(&e)),
            account: label.clone(),
            id: p.id.clone(),
            title: e.title,
            ..Default::default()
        })
    })
}

fn freebusy(x: &Ext, _: &Job, p: FreebusyParams) -> anyhow::Result<FreebusyResult> {
    let (from, to) = calendar::range(&p.from, &p.to)?;
    for e in &p.emails {
        if e.contains(['\r', '\n']) || e.trim().is_empty() {
            bail!("emails: {e:?} isn't an address or a calendar's id");
        }
    }
    let labels = x.reads(&p.account)?;
    let ids: Vec<String> = if p.emails.is_empty() { vec!["primary".into()] } else { p.emails.clone() };
    let mut out = FreebusyResult::default();
    for label in &labels {
        let items: Vec<Value> = ids.iter().map(|id| json!({"id": id})).collect();
        let got = x.api(label, |api| {
            api.send_json(
                "POST",
                &api.calendar("freeBusy"),
                &[],
                &json!({"timeMin": from, "timeMax": to, "items": items}),
            )
        });
        match got {
            Ok(v) => {
                for id in &ids {
                    let c = &v["calendars"][id.as_str()];
                    let busy = c["busy"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|b| Span { start: s(&b["start"]), end: s(&b["end"]), ..Default::default() })
                        .collect();
                    let error = c["errors"][0]["reason"].as_str().unwrap_or_default().to_string();
                    out.calendars.push(Busy {
                        account: label.clone(),
                        email: id.clone(),
                        busy,
                        error,
                        ..Default::default()
                    });
                }
            }
            Err(e) => out.failed.push(problem(label, &e)),
        }
    }
    all_failed(&labels, &out.failed)?;
    let mut text = format!("Busy times from {} to {}:", p.from, p.to);
    for b in &out.calendars {
        let whose = if labels.len() > 1 { format!("{} (asked as {})", b.email, b.account) } else { b.email.clone() };
        let said = if !b.error.is_empty() {
            format!("Google couldn't say ({})", b.error)
        } else if b.busy.is_empty() {
            "free".to_string()
        } else {
            let spans: Vec<String> = b.busy.iter().map(|s| format!("{} to {}", s.start, s.end)).collect();
            spans.join(", ")
        };
        text.push_str(&format!("\n- {whose}: {said}"));
    }
    text.push_str(&failures(&out.failed));
    out.text = text;
    Ok(out)
}

fn main() -> anyhow::Result<()> {
    let mut ext = Extension::new()?;
    let x = Arc::new(Ext { h: ext.handle(), agent: agent(), ends: Endpoints::from_env(), tokens: Mutex::default() });
    let signins = Arc::new(SignIns::default());

    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: AccountsParams| accounts_list(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: MailSearchParams| mail_search(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: MailReadParams| mail_read(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: MailDraftParams| mail_draft(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: MailSendParams| mail_send(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: CalendarsParams| calendars(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: EventsParams| events(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: EventCreateParams| event_create(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: EventUpdateParams| event_update(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: EventDeleteParams| event_delete(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: FreebusyParams| freebusy(&xx, job, p));

    // `/google add` and `/google client` posted (an agent, an older app): the apps open the forms for them
    ext.typed_command(|_: &Job, _: AddParams| {
        Ok(AddResult {
            text: "A Google account is signed in from the Pointman app, so its token never goes in a post: type \
                   `/google add <label>` there and tap Sign in."
                .into(),
            ..Default::default()
        })
    });
    ext.typed_command(|_: &Job, _: ClientParams| {
        Ok(ClientResult {
            text: "The Google OAuth client is added in the Pointman app, so its secret never goes in a post: type \
                   `/google client` there and paste its client ID and secret."
                .into(),
            ..Default::default()
        })
    });

    // Sign in from the apps: Google's authorization-code flow with PKCE, back through core's /oauth/callback
    let (xx, si) = (x.clone(), signins.clone());
    ext.on(move |p: SignInRequest| {
        if p.form != "account" {
            bail!("the Google extension signs in accounts only (form: account); the OAuth client is pasted with /google client");
        }
        let label = p.label.trim();
        if label == CLIENT_ITEM.trim_start_matches("google.") {
            bail!("{CLIENT_ITEM} is the OAuth client's place in the vault: call the account something else");
        }
        let session = p.session.as_ref().map(|s| s.id.as_str()).unwrap_or_default();
        si.start(&xx.ends, xx.client()?, session, &p.redirect_uri, label)
    });
    let (xx, si) = (x.clone(), signins);
    ext.on(move |p: CredentialsCodeRequest| {
        let session = p.session.as_option().map(|s| s.id.as_str()).unwrap_or_default();
        let value = si.code(&xx.agent, &xx.ends, session, &p.code)?;
        Ok(CredentialsCodeResponse { value, ..Default::default() })
    });

    ext.run()
}
