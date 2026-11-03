//! The extension's commands under a stand-in node, against a fake Google (tests/common): sign-ins that come back by
//! redirect with PKCE, reads across every account that say which they used, writes that stop rather than pick an
//! account, and nothing sent or changed without `confirm`. No real Google is called: the extension is pointed at the
//! fake through GOOGLE_GMAIL_BASE, GOOGLE_CALENDAR_BASE and GOOGLE_OAUTH_BASE.

mod common;

use common::{Fake, CLIENT_ID, CLIENT_SECRET, CODE, HOME_REFRESH, NEW_REFRESH, WORK_REFRESH};
use pointman_extension::testing::StandIn;
use serde_json::{json, Value};

const REDIRECT: &str = "https://board.example.com/oauth/callback";

fn node_with(fake: &Fake, settings: Value, secrets: Vec<(&str, String)>) -> StandIn {
    StandIn::new(env!("CARGO_MANIFEST_DIR"))
        .binary(env!("CARGO_BIN_EXE_google"))
        .secrets(secrets)
        .settings(settings)
        .env("GOOGLE_GMAIL_BASE", &fake.url)
        .env("GOOGLE_CALENDAR_BASE", &fake.url)
        .env("GOOGLE_OAUTH_BASE", &fake.url)
        .start()
        .unwrap()
}

fn client() -> (&'static str, String) {
    ("google.client", format!("{CLIENT_ID} {CLIENT_SECRET}"))
}

/// Two accounts signed in, work and home.
fn both(fake: &Fake) -> StandIn {
    node_with(
        fake,
        json!({}),
        vec![client(), ("google.work", WORK_REFRESH.to_string()), ("google.home", HOME_REFRESH.to_string())],
    )
}

fn query_of(url: &str, key: &str) -> String {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or_default();
    query.split('&').find_map(|kv| kv.strip_prefix(&format!("{key}="))).unwrap_or_default().to_string()
}

fn sign_in(node: &StandIn, session: &str, label: &str) -> Result<Value, pointman_extension::testing::Failed> {
    node.call(
        "credentials.sign_in",
        json!({"form": "account", "label": label, "person": {"id": "mem_1", "name": "james"},
            "session": {"id": session}, "redirectUri": REDIRECT}),
    )
}

#[test]
fn a_sign_in_comes_back_by_redirect_and_is_exchanged_with_its_pkce_verifier() {
    let fake = Fake::start();
    let node = node_with(&fake, json!({}), vec![client()]);
    let started = sign_in(&node, "sgn_1", "work").unwrap();
    let url = started["url"].as_str().unwrap();
    assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"), "{url}");
    assert_eq!(query_of(url, "client_id"), CLIENT_ID);
    assert_eq!(query_of(url, "redirect_uri"), "https%3A%2F%2Fboard.example.com%2Foauth%2Fcallback");
    assert_eq!(query_of(url, "access_type"), "offline");
    assert_eq!(query_of(url, "code_challenge_method"), "S256");
    let scope = query_of(url, "scope");
    for s in ["openid", "email", "gmail.modify", "gmail.send", "auth%2Fcalendar"] {
        assert!(scope.contains(s), "{s} in {scope}");
    }
    assert!(query_of(url, "prompt").contains("consent"));
    let state = started["state"].as_str().unwrap();
    assert!((16..=128).contains(&state.len()), "core takes 16 to 128 characters: {state}");
    assert!(state.chars().all(|c| c.is_ascii_alphanumeric() || "._~-".contains(c)), "{state}");
    assert_eq!(query_of(url, "state"), state);
    assert_eq!(started["waits"], json!(false), "a redirect sign-in doesn't wait");
    assert!(started.get("code").is_none(), "nothing to show or paste");

    // core's callback gives the node Google's code: exchanged with the verifier whose challenge was in the link
    {
        let mut s = fake.state();
        s.challenge = query_of(url, "code_challenge");
        s.redirect = REDIRECT.into();
    }
    let got =
        node.call("credentials.code", json!({"form": "account", "session": {"id": "sgn_1"}, "code": CODE})).unwrap();
    assert_eq!(got["value"], NEW_REFRESH, "the vault's value is the refresh token alone");
    let exchange = fake
        .requests()
        .into_iter()
        .find(|r| r.form.get("grant_type").map(String::as_str) == Some("authorization_code"));
    let exchange = exchange.expect("the code was exchanged");
    assert_eq!(exchange.form["redirect_uri"], REDIRECT, "with the same redirect");
    assert_eq!(exchange.form["code"], CODE);

    // used once: the sign-in is over
    let e = node
        .call("credentials.code", json!({"form": "account", "session": {"id": "sgn_1"}, "code": CODE}))
        .unwrap_err();
    assert!(e.message.contains("ended"), "{e:?}");

    // nothing about the code, the verifier or the token in its log
    let log = node.log();
    for secret in [CODE, NEW_REFRESH, CLIENT_SECRET, exchange.form["code_verifier"].as_str()] {
        assert!(!log.contains(secret), "{secret} in the log: {log}");
    }
}

#[test]
fn a_code_with_another_verifier_or_redirect_is_refused() {
    let fake = Fake::start();
    let node = node_with(&fake, json!({}), vec![client()]);
    sign_in(&node, "sgn_1", "work").unwrap();
    // the fake expects another sign-in's challenge: Google says invalid_grant
    {
        let mut s = fake.state();
        s.challenge = "not-this-one".into();
        s.redirect = REDIRECT.into();
    }
    let e = node
        .call("credentials.code", json!({"form": "account", "session": {"id": "sgn_1"}, "code": CODE}))
        .unwrap_err();
    assert!(e.message.contains("start the sign-in again"), "{e:?}");
    assert!(!e.message.contains(CODE), "the code isn't in the error");
    // a session it never started
    let e = node
        .call("credentials.code", json!({"form": "account", "session": {"id": "sgn_9"}, "code": CODE}))
        .unwrap_err();
    assert!(e.message.contains("ended"), "{e:?}");
}

#[test]
fn a_token_answer_that_isnt_json_is_left_out_of_the_error() {
    // a cut-off answer and an error page, each holding tokens: a sign-in's exchange and a refresh say neither
    let leaked = "ya29.LEAKEDaccessTOKENfor-the-tests";
    let answers = [
        (200, format!(r#"{{"access_token": "{leaked}", "refresh_token": "{NEW_REFRESH}", "expires_in": 35"#)),
        (502, format!("<html><body>upstream: {leaked} {NEW_REFRESH}</body></html>")),
    ];
    for (status, body) in answers {
        let fake = Fake::start();
        fake.state().garbled = Some((status, body));
        let node = node_with(&fake, json!({}), vec![client(), ("google.work", WORK_REFRESH.to_string())]);
        sign_in(&node, "sgn_1", "work").unwrap();
        let exchange = node
            .call("credentials.code", json!({"form": "account", "session": {"id": "sgn_1"}, "code": CODE}))
            .unwrap_err();
        let refresh = node.run("google.mail_search", json!({"query": "x", "account": "work"})).unwrap_err();
        for e in [exchange.message, refresh.message] {
            assert!(e.contains(&format!("Google said {status}")) && e.contains("left out"), "{e}");
            for secret in [leaked, NEW_REFRESH, WORK_REFRESH, CODE] {
                assert!(!e.contains(secret), "{secret} in the error: {e}");
            }
        }
        let log = node.log();
        assert!(!log.contains(leaked) && !log.contains(NEW_REFRESH), "a token in the log: {log}");
    }
}

#[test]
fn a_sign_in_that_leaves_out_a_scope_is_refused() {
    let fake = Fake::start();
    let node = node_with(&fake, json!({}), vec![client()]);
    let url = sign_in(&node, "sgn_1", "home").unwrap()["url"].as_str().unwrap().to_string();
    {
        let mut s = fake.state();
        s.challenge = query_of(&url, "code_challenge");
        s.redirect = REDIRECT.into();
        s.granted =
            "openid https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/calendar".into();
    }
    let e = node
        .call("credentials.code", json!({"form": "account", "session": {"id": "sgn_1"}, "code": CODE}))
        .unwrap_err();
    assert!(e.message.contains("gmail.send") && e.message.contains("sign home in again"), "{e:?}");
}

#[test]
fn a_sign_in_needs_a_redirect_the_client_and_a_label_thats_not_the_clients() {
    let fake = Fake::start();
    let node = node_with(&fake, json!({}), vec![client()]);
    // an older node sends no redirect
    let e = node
        .call("credentials.sign_in", json!({"form": "account", "label": "work", "session": {"id": "sgn_1"}}))
        .unwrap_err();
    assert!(e.message.contains("pointman upgrade"), "{e:?}");
    // the field's own name is read too
    let ok = node.call(
        "credentials.sign_in",
        json!({"form": "account", "label": "work", "session": {"id": "sgn_2"}, "redirect_uri": REDIRECT}),
    );
    assert!(ok.is_ok(), "{ok:?}");
    let e = sign_in(&node, "sgn_3", "client").unwrap_err();
    assert!(e.message.contains("google.client"), "{e:?}");
    let e = node
        .call("credentials.sign_in", json!({"form": "client", "session": {"id": "sgn_4"}, "redirectUri": REDIRECT}))
        .unwrap_err();
    assert!(e.message.contains("/google client"), "{e:?}");

    let bare = node_with(&fake, json!({}), vec![]);
    let e = sign_in(&bare, "sgn_1", "work").unwrap_err();
    assert!(e.message.contains("/google client"), "{e:?}");
    assert!(fake.requests().is_empty(), "no call to Google: {:?}", fake.requests());
}

#[test]
fn accounts_say_each_address_and_whether_its_sign_in_works() {
    let fake = Fake::start();
    fake.state().revoked.push(HOME_REFRESH.into());
    let node = node_with(
        &fake,
        json!({"account": "work"}),
        vec![client(), ("google.work", WORK_REFRESH.to_string()), ("google.home", HOME_REFRESH.to_string())],
    );
    let done = node.run("google.accounts", json!({})).unwrap();
    let accounts = done.result["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2, "the client isn't an account: {accounts:?}");
    let home = &accounts[0];
    // proto's JSON leaves a false out
    assert_eq!((home["label"].as_str(), home["works"].as_bool().unwrap_or(false)), (Some("home"), false));
    assert!(home["error"].as_str().unwrap().contains("/google add home"), "{home}");
    let work = &accounts[1];
    assert_eq!(work["email"], "james@work.example.com");
    assert_eq!((work["works"].as_bool(), work["chosen"].as_bool()), (Some(true), Some(true)));
    let text = done.result["text"].as_str().unwrap();
    assert!(text.contains("work (james@work.example.com)") && text.contains("home (not working"), "{text}");
    assert!(!text.contains(HOME_REFRESH) && !text.contains(WORK_REFRESH));
}

#[test]
fn a_read_with_several_accounts_asks_them_all_and_says_which_it_used() {
    let fake = Fake::start();
    let node = both(&fake);
    let done = node.run("google.mail_search", json!({"query": "in:inbox"})).unwrap();
    let r = &done.result;
    assert_eq!(r["accounts"], json!(["home", "work"]));
    let found: Vec<(String, String)> = r["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["account"].as_str().unwrap().to_string(), m["id"].as_str().unwrap().to_string()))
        .collect();
    let want: Vec<(String, String)> =
        [("work", "w1"), ("home", "h1"), ("work", "w2")].iter().map(|(a, i)| (a.to_string(), i.to_string())).collect();
    assert_eq!(found, want, "both accounts' messages, newest first");
    let text = r["text"].as_str().unwrap();
    assert!(text.contains("home and work") && text.contains("Quarterly plan") && text.contains("[work w1]"), "{text}");
    assert!(!text.contains("Secret numbers"), "no snippet in the text: {text}");

    // one named: only its own
    let done = node.run("google.mail_search", json!({"query": "in:inbox", "account": "home", "limit": 5})).unwrap();
    assert_eq!(done.result["accounts"], json!(["home"]));
    assert_eq!(done.result["messages"].as_array().unwrap().len(), 1);
    let list = fake.api_requests().into_iter().rev().find(|r| r.path == "/gmail/v1/users/me/messages").unwrap();
    assert_eq!((list.account.as_str(), list.q("maxResults"), list.q("q")), ("home", Some("5"), Some("in:inbox")));

    // one that isn't signed in
    let e = node.run("google.mail_search", json!({"query": "x", "account": "school"})).unwrap_err();
    assert!(e.message.contains("home, work") && e.message.contains("/google add school"), "{e:?}");
}

#[test]
fn a_message_is_read_with_its_words_kept_out_of_the_text() {
    let fake = Fake::start();
    let node = both(&fake);
    // no account named: each is asked until one has it
    let done = node.run("google.mail_read", json!({"id": "w1"})).unwrap();
    let r = &done.result;
    assert_eq!(r["account"], "work");
    assert_eq!(r["body"], "The plan is attached. Secret numbers inside.");
    assert_eq!(r["subject"], "Quarterly plan");
    assert_eq!(r["message_id"], "<w1@mail.example.com>");
    assert_eq!(r["attachments"][0]["name"], "notes.pdf");
    let text = r["text"].as_str().unwrap();
    assert!(text.contains("Quarterly plan") && text.contains("notes.pdf"), "{text}");
    assert!(!text.contains("Secret numbers"), "the body stays out of the text: {text}");

    let e = node.run("google.mail_read", json!({"id": "nope1"})).unwrap_err();
    assert!(e.message.contains("no message nope1"), "{e:?}");
}

#[test]
fn a_write_with_several_accounts_and_none_named_stops_and_lists_them() {
    let fake = Fake::start();
    let node = both(&fake);
    let e = node
        .run("google.mail_draft", json!({"to": ["bob@example.com"], "subject": "Hi", "body": "Hello"}))
        .unwrap_err();
    assert!(e.message.contains("2 Google accounts") && e.message.contains("home, work"), "{e:?}");
    let e = node.run("google.event_delete", json!({"id": "ew1", "confirm": true})).unwrap_err();
    assert!(e.message.contains("account=<label>"), "{e:?}");
    assert!(fake.api_requests().is_empty(), "nothing reached Google: {:?}", fake.api_requests());

    // the setting chooses one
    let node = node_with(
        &fake,
        json!({"account": "work"}),
        vec![client(), ("google.work", WORK_REFRESH.to_string()), ("google.home", HOME_REFRESH.to_string())],
    );
    let done =
        node.run("google.mail_draft", json!({"to": ["bob@example.com"], "subject": "Hi", "body": "Hello"})).unwrap();
    assert_eq!(done.result["account"], "work");
    assert_eq!(fake.mailbox("work").drafts.len(), 1);
}

#[test]
fn a_draft_reply_goes_in_its_thread_and_nothing_is_sent() {
    let fake = Fake::start();
    let node = both(&fake);
    let done = node
        .run("google.mail_draft", json!({"reply_to": "w2", "body": "Friday works.\nSee you.", "account": "work"}))
        .unwrap();
    let r = &done.result;
    assert_eq!((r["subject"].as_str(), r["thread_id"].as_str()), (Some("Re: Lunch?"), Some("tw2")));
    assert_eq!(r["to"], json!(["Ann <ann@example.com>"]));
    let work = fake.mailbox("work");
    assert!(work.sent.is_empty(), "a draft isn't sent");
    let raw = work.drafts[0]["message"]["raw"].as_str().unwrap();
    let headers = common::raw_headers(raw);
    let header = |k: &str| headers.iter().find(|h| h["name"] == k).map(|h| h["value"].as_str().unwrap().to_string());
    assert_eq!(header("To").as_deref(), Some("Ann <ann@example.com>"));
    assert_eq!(header("In-Reply-To").as_deref(), Some("<w2@mail.example.com>"));
    assert_eq!(header("References").as_deref(), Some("<first@mail.example.com> <w2@mail.example.com>"));
    assert_eq!(common::raw_body(raw), "Friday works.\r\nSee you.");
    assert_eq!(work.drafts[0]["message"]["threadId"], "tw2");
}

#[test]
fn mail_is_sent_only_with_confirm() {
    let fake = Fake::start();
    let node = both(&fake);
    let params = json!({"to": ["bob@example.com"], "subject": "Notes", "body": "Here they are.", "account": "work"});
    for confirm in [json!(null), json!(false)] {
        let mut p = params.clone();
        if !confirm.is_null() {
            p["confirm"] = confirm;
        }
        let e = node.run("google.mail_send", p).unwrap_err();
        assert!(e.message.contains("confirm=true") && e.message.contains("\"Notes\" to bob@example.com"), "{e:?}");
    }
    assert!(fake.requests().is_empty(), "no token asked for, nothing sent: {:?}", fake.requests());

    let mut p = params.clone();
    p["confirm"] = json!(true);
    let done = node.run("google.mail_send", p).unwrap();
    assert!(done.result["text"].as_str().unwrap().starts_with("Sent \"Notes\" to bob@example.com from work"));
    let sent = fake.mailbox("work").sent;
    assert_eq!(common::raw_body(sent[0]["raw"].as_str().unwrap()), "Here they are.");

    // a draft, sent as it is
    let drafted = node
        .run(
            "google.mail_draft",
            json!({"to": ["ann@example.com"], "subject": "Plan", "body": "Attached.", "account": "work"}),
        )
        .unwrap();
    let draft = drafted.result["draft"].as_str().unwrap().to_string();
    let e = node.run("google.mail_send", json!({"draft": draft, "account": "work"})).unwrap_err();
    assert!(e.message.contains(&format!("draft {draft}")), "{e:?}");
    let e = node
        .run("google.mail_send", json!({"draft": draft, "subject": "x", "account": "work", "confirm": true}))
        .unwrap_err();
    assert!(e.message.contains("not both"), "{e:?}");
    let done = node.run("google.mail_send", json!({"draft": draft, "account": "work", "confirm": true})).unwrap();
    assert_eq!(done.result["to"], json!(["ann@example.com"]));
    assert_eq!(done.result["subject"], "Plan");
    assert_eq!(fake.mailbox("work").sent.len(), 2);
    assert!(fake.mailbox("work").drafts.is_empty());
}

#[test]
fn events_come_from_every_account_soonest_first() {
    let fake = Fake::start();
    let node = both(&fake);
    let done = node.run("google.events", json!({"from": "2026-11-03", "to": "2026-11-10"})).unwrap();
    let titles: Vec<&str> =
        done.result["events"].as_array().unwrap().iter().map(|e| e["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Dentist", "Standup"], "home's first: it's sooner");
    assert_eq!(done.result["events"][0]["account"], "home");
    let text = done.result["text"].as_str().unwrap();
    assert!(text.contains("- home: 2026-11-03T15:00:00+08:00"), "{text}");
    let asked = fake.api_requests().into_iter().find(|r| r.path == "/calendar/v3/calendars/primary/events").unwrap();
    assert_eq!(asked.q("timeMin"), Some("2026-11-03T00:00:00Z"));
    assert_eq!(asked.q("timeMax"), Some("2026-11-10T00:00:00Z"));
    assert_eq!((asked.q("singleEvents"), asked.q("orderBy")), (Some("true"), Some("startTime")));

    let e = node.run("google.events", json!({"from": "2026-11-10", "to": "2026-11-03"})).unwrap_err();
    assert!(e.message.contains("isn't after"), "{e:?}");
    let e = node.run("google.events", json!({"from": "next tuesday", "to": "2026-11-03"})).unwrap_err();
    assert!(e.message.contains("isn't a time"), "{e:?}");

    let cals = node.run("google.calendars", json!({"account": "work"})).unwrap();
    assert_eq!(cals.result["calendars"][0]["id"], "james@work.example.com");
    assert_eq!(cals.result["calendars"][0]["primary"], json!(true));
}

#[test]
fn an_event_is_made_changed_and_deleted_only_with_confirm() {
    let fake = Fake::start();
    let node = both(&fake);
    let make = json!({"title": "Review", "start": "2026-11-05T10:00", "end": "2026-11-05T10:30", "time_zone": "Asia/Singapore",
        "attendees": ["bob@example.com"], "meet": true, "notify": true, "account": "work"});
    let e = node.run("google.event_create", make.clone()).unwrap_err();
    assert!(e.message.contains("confirm=true") && e.message.contains("inviting bob@example.com"), "{e:?}");
    assert!(fake.requests().is_empty(), "nothing reached Google: {:?}", fake.requests());

    let mut yes = make.clone();
    yes["confirm"] = json!(true);
    let made = node.run("google.event_create", yes).unwrap();
    let ev = &made.result["event"];
    assert_eq!(ev["meet"], "https://meet.google.com/abc-defg-hij");
    let id = ev["id"].as_str().unwrap().to_string();
    let insert = fake.api_requests().into_iter().find(|r| r.method == "POST").unwrap();
    assert_eq!((insert.q("sendUpdates"), insert.q("conferenceDataVersion")), (Some("all"), Some("1")));
    assert_eq!(insert.json["start"], json!({"dateTime": "2026-11-05T10:00:00", "timeZone": "Asia/Singapore"}));
    assert_eq!(insert.json["attendees"], json!([{"email": "bob@example.com"}]));

    let e =
        node.run("google.event_update", json!({"id": id, "title": "Review (moved)", "account": "work"})).unwrap_err();
    assert!(e.message.contains("confirm=true") && e.message.contains("title"), "{e:?}");
    let e = node
        .run(
            "google.event_update",
            json!({"id": id, "start": "2026-11-05T11:00:00Z", "account": "work", "confirm": true}),
        )
        .unwrap_err();
    assert!(e.message.contains("together"), "{e:?}");
    let changed = node
        .run("google.event_update", json!({"id": id, "title": "Review (moved)", "account": "work", "confirm": true}))
        .unwrap();
    assert_eq!(changed.result["changed"], json!(["title"]));
    let patch = fake.api_requests().into_iter().find(|r| r.method == "PATCH").unwrap();
    assert_eq!(patch.json, json!({"summary": "Review (moved)"}), "only what changes is sent");
    assert_eq!(patch.q("sendUpdates"), Some("none"));

    let e = node.run("google.event_delete", json!({"id": id, "account": "work"})).unwrap_err();
    assert!(e.message.contains("confirm=true"), "{e:?}");
    let gone = node.run("google.event_delete", json!({"id": id, "account": "work", "confirm": true})).unwrap();
    assert_eq!(gone.result["title"], "Review (moved)");
    assert!(fake.mailbox("work").events.iter().all(|e| e["id"] != id.as_str()));
}

#[test]
fn freebusy_says_when_each_calendar_is_busy() {
    let fake = Fake::start();
    let node = both(&fake);
    let done = node
        .run(
            "google.freebusy",
            json!({"from": "2026-11-04T00:00:00Z", "to": "2026-11-05T00:00:00Z", "emails": ["primary", "nobody@example.com"], "account": "work"}),
        )
        .unwrap();
    let cals = done.result["calendars"].as_array().unwrap();
    assert_eq!(cals[0]["busy"][0]["start"], "2026-11-04T01:00:00Z");
    assert_eq!(cals[1]["error"], "notFound");
    let text = done.result["text"].as_str().unwrap();
    assert!(text.contains("nobody@example.com: Google couldn't say (notFound)"), "{text}");
}

#[test]
fn params_it_doesnt_take_fail_before_anything_runs() {
    let fake = Fake::start();
    let node = both(&fake);
    let e = node.run("google.mail_search", json!({})).unwrap_err();
    assert_eq!(e.message, "query is missing");
    let e = node.run("google.mail_search", json!({"query": "x", "acount": "work"})).unwrap_err();
    assert!(e.message.contains("acount isn't a field"), "{e:?}");
    let e = node
        .run("google.mail_draft", json!({"to": ["bob"], "subject": "Hi", "body": "x", "account": "work"}))
        .unwrap_err();
    assert!(e.message.contains("isn't an address"), "{e:?}");
    let e = node
        .run(
            "google.mail_draft",
            json!({"to": ["bob@example.com"], "subject": "Hi\r\nBcc: eve@example.com", "body": "x", "account": "work"}),
        )
        .unwrap_err();
    assert!(e.message.contains("line break"), "{e:?}");
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
}
