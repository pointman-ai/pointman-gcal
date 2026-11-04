//! The extension's process: the node starts it (`bin/gcal`, from the extension's folder) and talks to it over stdin
//! and stdout. It lists calendars, events and busy times, makes, changes and deletes events, and signs Google accounts
//! in from the apps, with Pointman's shared Google client where core has one and the vault's `gcal.client` where it
//! doesn't. Nothing reaches Google without a sign-in in the vault; no event is made, changed or deleted without
//! `confirm`; and no token or code is ever logged.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use gcal::accounts::{self, item};
use gcal::google::{access_token, agent, refusal, segment, shared_access_token, Api, Endpoints, OAuthClient};
use gcal::signin::{self, Client, SignIns, UPGRADE};
use gcal::v1::{
    Account, AccountsParams, AccountsResult, AddParams, AddResult, Busy, Calendar, CalendarsParams, CalendarsResult,
    ClientParams, ClientResult, CreateParams, CreateResult, DeleteParams, DeleteResult, EventsParams, EventsResult,
    FreebusyParams, FreebusyResult, Problem, Settings, Span, UpdateParams, UpdateResult,
};
use gcal::{and, calendar, count, CLIENT_ITEM, PROVIDER, SHARED};
use pointman_extension::protocol::{CredentialsCodeRequest, CredentialsCodeResponse, CredentialsSignInRequest};
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
        Ok(accounts::labels(self.h.secrets("gcal.")?))
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

    /// A self-hosted Pointman's own Web application client: the vault's `gcal.client`.
    fn client(&self) -> anyhow::Result<OAuthClient> {
        let value = self.h.secret(CLIENT_ITEM).map_err(|_| {
            anyhow!(
                "no Google OAuth client of your own yet, which a Pointman without a shared one needs: type /gcal \
                 client in the app and paste a Web application client's ID and secret"
            )
        })?;
        OAuthClient::parse(&value).ok_or_else(|| {
            anyhow!(
                "{CLIENT_ITEM} isn't a client ID and secret (…apps.googleusercontent.com, a space, then the secret): \
                 add it again with /gcal client"
            )
        })
    }

    /// The client a sign-in sent to `redirect` goes through: core's shared one when it has one, and only if that's
    /// where `redirect` points; the vault's `gcal.client` when core says it has none. Anything else stops the
    /// sign-in, with no falling back to the other.
    fn sign_in_client(&self, redirect: &str) -> anyhow::Result<Client> {
        let redirect = redirect.trim();
        if redirect.is_empty() {
            bail!(UPGRADE);
        }
        let core = self.h.oauth_shared_client(PROVIDER).map_err(|e| {
            anyhow!(
                "Pointman couldn't say whether it has a shared Google client, so the sign-in stops: {}",
                refusal(&e)
            )
        })?;
        if !core.shared {
            return Ok(Client::Own(self.client()?));
        }
        if core.client_id.trim().is_empty() {
            bail!("Pointman's shared Google client came with no id, so the sign-in stops: start it again");
        }
        if core.redirect_uri.trim() != redirect {
            bail!(
                "the sign-in was given {redirect} to come back to, but Pointman's shared Google client comes back to \
                 {}, so it stops: start it again",
                core.redirect_uri
            );
        }
        Ok(Client::Shared { id: core.client_id.trim().to_string() })
    }

    /// An access token for an account, made again when it's within a minute of running out, or `fresh`: through core
    /// when Pointman's shared client signed it in (`shared:` before its refresh token), else with `gcal.client`.
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
            .ok_or_else(|| anyhow!("{name} isn't a Google refresh token: sign it in again with /gcal add {label}"))?;
        let (token, lasts) = if value.trim_start().starts_with(SHARED) {
            shared_access_token(&self.h, &refresh, &name)?
        } else {
            access_token(&self.agent, &self.ends, &self.client()?, &refresh, &name)?
        };
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
            text: "No Google account is signed in: /gcal add <label> in the app signs one in. A self-hosted \
                   Pointman needs its own OAuth client first: /gcal client."
                .into(),
            ..Default::default()
        });
    }
    let setting = x.setting();
    let mut out = AccountsResult::default();
    let mut lines = Vec::new();
    for label in &have {
        let mut a = Account { label: label.clone(), chosen: *label == setting, ..Default::default() };
        // the account's own calendar's id is its address
        match x.api(label, |api| api.get(&api.calendar("calendars/primary"), &[])) {
            Ok(v) => {
                a.email = s(&v["id"]);
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

fn create(x: &Ext, _: &Job, p: CreateParams) -> anyhow::Result<CreateResult> {
    let zone = p.time_zone.trim();
    let (start, end) = calendar::times(&p.start, &p.end, zone)?;
    calendar::check_addresses("attendees", &p.attendees)?;
    let cal = calendar_id(&p.calendar);
    let label = x.write(&p.account, "gcal.create")?;
    if !p.confirm {
        let invites = if p.attendees.is_empty() { String::new() } else { format!(", inviting {}", and(&p.attendees)) };
        bail!(
            "not made: gcal.create needs confirm=true, given only once the person has said yes to \"{}\" from \
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
        Ok(CreateResult {
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

fn update(x: &Ext, _: &Job, p: UpdateParams) -> anyhow::Result<UpdateResult> {
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
        calendar::check_addresses("attendees", &p.attendees)?;
        body["attendees"] = json!(p.attendees.iter().map(|a| json!({"email": a})).collect::<Vec<_>>());
        changed.push("attendees".into());
    }
    if changed.is_empty() {
        bail!("nothing to change: give title, start and end, description, location or attendees");
    }
    let cal = calendar_id(&p.calendar);
    let label = x.write(&p.account, "gcal.update")?;
    if !p.confirm {
        bail!(
            "not changed: gcal.update needs confirm=true, given only once the person has said yes to changing \
             the {} of event {} on {label}'s {cal} calendar",
            and(&changed),
            p.id
        );
    }
    x.api(&label, |api| {
        let url = api.calendar(&format!("calendars/{}/events/{}", segment(&cal), segment(&p.id)));
        let after = api.send_json("PATCH", &url, &[("sendUpdates", updates(p.notify))], &body)?;
        let e = calendar::event(&label, &cal, &after);
        Ok(UpdateResult {
            text: format!("Changed the {} of \"{}\": {}.", and(&changed), e.title, calendar::line(&e)),
            event: e.into(),
            changed: changed.clone(),
            ..Default::default()
        })
    })
}

fn delete(x: &Ext, _: &Job, p: DeleteParams) -> anyhow::Result<DeleteResult> {
    let cal = calendar_id(&p.calendar);
    let label = x.write(&p.account, "gcal.delete")?;
    if !p.confirm {
        bail!(
            "not deleted: gcal.delete needs confirm=true, given only once the person has said yes to deleting \
             event {} from {label}'s {cal} calendar",
            p.id
        );
    }
    x.api(&label, |api| {
        let url = api.calendar(&format!("calendars/{}/events/{}", segment(&cal), segment(&p.id)));
        let e = calendar::event(&label, &cal, &api.get(&url, &[])?);
        api.delete(&url, &[("sendUpdates", updates(p.notify))])?;
        Ok(DeleteResult {
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
    ext.typed_command(move |job: &Job, p: CalendarsParams| calendars(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: EventsParams| events(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: CreateParams| create(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: UpdateParams| update(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: DeleteParams| delete(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: FreebusyParams| freebusy(&xx, job, p));

    // `/gcal add` and `/gcal client` posted (an agent, an older app): the apps open the forms for them
    ext.typed_command(|_: &Job, _: AddParams| {
        Ok(AddResult {
            text: "A Google account is signed in from the Pointman app, so its token never goes in a post: type \
                   `/gcal add <label>` there and tap Sign in."
                .into(),
            ..Default::default()
        })
    });
    ext.typed_command(|_: &Job, _: ClientParams| {
        Ok(ClientResult {
            text: "A self-hosted Pointman's own Google OAuth client is added in the Pointman app, so its secret never \
                   goes in a post: type `/gcal client` there and paste its client ID and secret. A hosted Pointman \
                   needs none: its sign-ins use Pointman's shared client."
                .into(),
            ..Default::default()
        })
    });

    // Sign in from the apps: Google's authorization-code flow with PKCE, back by redirect, with the shared client where
    // core has one
    let (xx, si) = (x.clone(), signins.clone());
    ext.on(move |p: CredentialsSignInRequest| {
        if p.form != "account" {
            bail!("gcal signs in accounts only (form: account); the OAuth client is pasted with /gcal client");
        }
        let label = p.label.trim();
        if label == CLIENT_ITEM.trim_start_matches("gcal.") {
            bail!("{CLIENT_ITEM} is the OAuth client's place in the vault: call the account something else");
        }
        let session = p.session.as_option().map(|s| s.id.as_str()).unwrap_or_default();
        si.start(&xx.ends, xx.sign_in_client(&p.redirect_uri)?, session, &p.redirect_uri, label)
    });
    let (xx, si) = (x.clone(), signins);
    ext.on(move |p: CredentialsCodeRequest| {
        let session = p.session.as_option().map(|s| s.id.as_str()).unwrap_or_default();
        let value = si.code(&xx.h, &xx.agent, &xx.ends, session, &p.code)?;
        Ok(CredentialsCodeResponse { value, ..Default::default() })
    });

    ext.run()
}
