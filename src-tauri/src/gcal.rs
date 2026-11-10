//! Google Calendar, two-way (owner 2026-11-09, D047). The owner's own Google Cloud OAuth client (type "Desktop
//! app") signs in through the system browser with PKCE and a one-shot 127.0.0.1 callback; the refresh token stays in
//! the app's database (settings key `gcal`, never sent to the page). Each workspace can map to a Google calendar the
//! owner shares with the team (D048); its open due dates are kept there as all-day events tagged with the record id.

use crate::{db, AppState};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::time::Duration;
use tauri::State;

type R<T> = Result<T, String>;

const AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const LIST: &str = "https://www.googleapis.com/calendar/v3/users/me/calendarList";
const SCOPE: &str = "https://www.googleapis.com/auth/calendar.events https://www.googleapis.com/auth/calendar.calendarlist.readonly";

static ACCESS: Mutex<Option<(String, i64)>> = Mutex::new(None); // (token, expires at, unix secs)

fn cfg(st: &State<AppState>) -> R<Value> {
    let c = st.db.lock().map_err(|_| "database lock poisoned")?;
    Ok(db::setting(&c, "gcal").unwrap_or_else(|| json!({})))
}
fn save_cfg(st: &State<AppState>, v: &Value) -> R<()> {
    let c = st.db.lock().map_err(|_| "database lock poisoned")?;
    db::set_setting(&c, "gcal", v).map_err(|e| e.to_string())
}
fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}
fn b64url(b: &[u8]) -> String {
    crate::hub_client::b64_encode(b).replace('+', "-").replace('/', "_").trim_end_matches('=').to_string()
}
fn random(n: usize) -> R<String> {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
    Ok(b64url(&b))
}
fn challenge(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).build()
}
/// Google's error text, without echoing anything we sent.
fn gerr(x: ureq::Error) -> String {
    match x {
        ureq::Error::Status(code, r) => {
            let v: Value = r.into_json().unwrap_or_default();
            let m = v["error"]["message"].as_str().or(v["error_description"].as_str()).or(v["error"].as_str()).unwrap_or("request refused");
            format!("Google said ({code}): {m}")
        }
        ureq::Error::Transport(t) => format!("Can't reach Google ({t})."),
    }
}

/// The `code` from the browser's redirect, if its `state` matches.
fn code_from(path: &str, state: &str) -> R<String> {
    let u = tauri::Url::parse(&format!("http://127.0.0.1{path}")).map_err(|_| "bad callback")?;
    let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
    if let Some(e) = q.get("error") {
        return Err(format!("Google sign-in was cancelled ({e})."));
    }
    if q.get("state").map(String::as_str) != Some(state) {
        return Err("Sign-in reply didn't match this request.".into());
    }
    q.get("code").cloned().ok_or_else(|| "Google sent no sign-in code.".into())
}

fn token(st: &State<AppState>) -> R<String> {
    if let Some((t, exp)) = ACCESS.lock().map_err(|_| "lock")?.clone() {
        if exp > now() + 60 {
            return Ok(t);
        }
    }
    let c = cfg(st)?;
    let refresh = s(&c, "refresh_token");
    if refresh.is_empty() {
        return Err("Connect Google Calendar first (CALENDAR › Connect).".into());
    }
    let v: Value = agent()
        .post(TOKEN)
        .send_form(&[("client_id", &s(&c, "client_id")), ("client_secret", &s(&c, "client_secret")), ("refresh_token", &refresh), ("grant_type", "refresh_token")])
        .map_err(gerr)?
        .into_json()
        .map_err(|e| e.to_string())?;
    let t = s(&v, "access_token");
    *ACCESS.lock().map_err(|_| "lock")? = Some((t.clone(), now() + v["expires_in"].as_i64().unwrap_or(3600)));
    Ok(t)
}

#[tauri::command(async)]
pub fn gcal_status(st: State<AppState>) -> R<Value> {
    let c = cfg(&st)?;
    Ok(json!({ "has_client": !s(&c, "client_id").is_empty(), "connected": !s(&c, "refresh_token").is_empty(), "connected_at": c["connected_at"], "team": c["team"] }))
}

/// Saves the OAuth client (from Google Cloud › Credentials › Desktop app). Replacing it signs out.
#[tauri::command(async)]
pub fn gcal_set_client(st: State<AppState>, client_id: String, client_secret: String) -> R<()> {
    let (id, secret) = (client_id.trim(), client_secret.trim());
    if !id.ends_with(".apps.googleusercontent.com") || secret.is_empty() {
        return Err("Paste the Client ID (ends in .apps.googleusercontent.com) and its Client secret.".into());
    }
    *ACCESS.lock().map_err(|_| "lock")? = None;
    save_cfg(&st, &json!({ "client_id": id, "client_secret": secret }))
}

/// Opens Google's sign-in in the browser and waits (up to 3 minutes) for it to come back to this PC.
#[tauri::command(async)]
pub fn gcal_connect(st: State<AppState>) -> R<()> {
    let mut c = cfg(&st)?;
    let id = s(&c, "client_id");
    if id.is_empty() {
        return Err("Add your Google OAuth client first.".into());
    }
    let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = server.server_addr().to_ip().map(|a| a.port()).ok_or("no port")?;
    let redirect = format!("http://127.0.0.1:{port}");
    let (verifier, state) = (random(48)?, random(24)?);
    let mut url = tauri::Url::parse(AUTH).map_err(|e| e.to_string())?;
    url.query_pairs_mut()
        .append_pair("client_id", &id)
        .append_pair("redirect_uri", &redirect)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &challenge(&verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("state", &state);
    crate::commands::shell_open(url.as_str())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    let code = loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let req = server.recv_timeout(left).map_err(|e| e.to_string())?.ok_or("Sign-in timed out. Try Connect again.")?;
        if req.url().starts_with("/favicon") {
            let _ = req.respond(tiny_http::Response::empty(404));
            continue;
        }
        let got = code_from(req.url(), &state);
        let page = if got.is_ok() { "Signed in. You can close this tab and go back to Command Center." } else { "Sign-in didn't finish. Go back to Command Center and try again." };
        let html = tiny_http::Header::from_bytes("Content-Type", "text/html; charset=utf-8").map_err(|_| "header")?;
        let _ = req.respond(tiny_http::Response::from_string(format!("<!doctype html><body style='background:#07050f;color:#ece8fb;font:16px sans-serif;padding:40px'>{page}</body>")).with_header(html));
        break got?;
    };
    let v: Value = agent()
        .post(TOKEN)
        .send_form(&[("code", code.as_str()), ("client_id", &id), ("client_secret", &s(&c, "client_secret")), ("redirect_uri", &redirect), ("grant_type", "authorization_code"), ("code_verifier", &verifier)])
        .map_err(gerr)?
        .into_json()
        .map_err(|e| e.to_string())?;
    let refresh = s(&v, "refresh_token");
    if refresh.is_empty() {
        return Err("Google didn't grant offline access. Try Connect again.".into());
    }
    c["refresh_token"] = json!(refresh);
    c["connected_at"] = json!(crate::util::now_iso());
    *ACCESS.lock().map_err(|_| "lock")? = Some((s(&v, "access_token"), now() + v["expires_in"].as_i64().unwrap_or(3600)));
    save_cfg(&st, &c)
}

#[tauri::command(async)]
pub fn gcal_disconnect(st: State<AppState>) -> R<()> {
    let mut c = cfg(&st)?;
    let refresh = s(&c, "refresh_token");
    if !refresh.is_empty() {
        let _ = agent().post("https://oauth2.googleapis.com/revoke").send_form(&[("token", &refresh)]);
    }
    c.as_object_mut().map(|m| m.remove("refresh_token"));
    *ACCESS.lock().map_err(|_| "lock")? = None;
    save_cfg(&st, &c)
}

/// Events of one calendar (empty = primary) between two RFC 3339 times (recurring events expanded).
#[tauri::command(async)]
pub fn gcal_events(st: State<AppState>, from: String, to: String, calendar: Option<String>) -> R<Vec<Value>> {
    let cal = calendar.unwrap_or_default();
    let v: Value = agent()
        .get(&events(&cal))
        .set("Authorization", &format!("Bearer {}", token(&st)?))
        .query("timeMin", &from)
        .query("timeMax", &to)
        .query("singleEvents", "true")
        .query("orderBy", "startTime")
        .query("maxResults", "500")
        .call()
        .map_err(gerr)?
        .into_json()
        .map_err(|e| e.to_string())?;
    Ok(v["items"].as_array().cloned().unwrap_or_default().into_iter().map(|e| json!({
        "id": e["id"], "title": e["summary"], "start": e["start"], "end": e["end"], "link": e["htmlLink"],
        "location": e["location"], "notes": e["description"], "ptcc": e["extendedProperties"]["private"]["ptcc"], "calendar": cal,
    })).collect())
}

/// Builds Google's start/end from the page's {date} (all day) or {dateTime}.
fn when(v: &Value) -> R<Value> {
    if let Some(d) = v["date"].as_str() {
        chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| "bad date")?;
        return Ok(json!({ "date": d }));
    }
    let t = v["dateTime"].as_str().ok_or("give a date or a time")?;
    chrono::DateTime::parse_from_rfc3339(t).map_err(|_| "bad time")?;
    Ok(json!({ "dateTime": t }))
}

/// Adds an event (no id) or changes one (id). Fields: title, start, end, notes, location.
#[tauri::command(async)]
pub fn gcal_save(st: State<AppState>, id: Option<String>, event: Value, calendar: Option<String>) -> R<String> {
    let url = events(&calendar.unwrap_or_default());
    let title = event["title"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("Give the event a title.")?;
    let mut body = json!({ "summary": title.chars().take(300).collect::<String>(), "start": when(&event["start"])?, "end": when(&event["end"])? });
    for (k, g) in [("notes", "description"), ("location", "location")] {
        if let Some(x) = event[k].as_str() {
            body[g] = json!(x.chars().take(4000).collect::<String>());
        }
    }
    let auth = format!("Bearer {}", token(&st)?);
    let r = match id.filter(|i| !i.is_empty()) {
        Some(i) => agent().request("PATCH", &format!("{url}/{}", enc(&i))).set("Authorization", &auth).send_json(body),
        None => agent().post(&url).set("Authorization", &auth).send_json(body),
    };
    let v: Value = r.map_err(gerr)?.into_json().map_err(|e| e.to_string())?;
    Ok(s(&v, "id"))
}

fn enc(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect()
}

#[tauri::command(async)]
pub fn gcal_delete(st: State<AppState>, id: String, calendar: Option<String>) -> R<()> {
    match agent().delete(&format!("{}/{}", events(&calendar.unwrap_or_default()), enc(&id))).set("Authorization", &format!("Bearer {}", token(&st)?)).call() {
        Ok(_) | Err(ureq::Error::Status(404 | 410, _)) => Ok(()),
        Err(x) => Err(gerr(x)),
    }
}

/// Writable calendars of the signed-in account (the owner creates one per workspace in Google Calendar).
#[tauri::command(async)]
pub fn gcal_calendars(st: State<AppState>) -> R<Vec<Value>> {
    let v: Value = agent().get(LIST).set("Authorization", &format!("Bearer {}", token(&st)?)).query("minAccessRole", "writer")
        .call().map_err(gerr)?.into_json().map_err(|e| e.to_string())?;
    Ok(v["items"].as_array().cloned().unwrap_or_default().into_iter()
        .map(|c| json!({ "id": c["id"], "name": c["summaryOverride"].as_str().or(c["summary"].as_str()), "primary": c["primary"], "color": c["backgroundColor"] }))
        .collect())
}

/// Workspace id -> Google calendar id (empty calendar = stop syncing that workspace).
#[tauri::command(async)]
pub fn gcal_set_team(st: State<AppState>, workspace: String, calendar: String) -> R<()> {
    let mut c = cfg(&st)?;
    if !c["team"].is_object() {
        c["team"] = json!({});
    }
    if calendar.trim().is_empty() { c["team"].as_object_mut().map(|m| m.remove(&workspace)); } else { c["team"][&workspace] = json!(calendar.trim()); }
    save_cfg(&st, &c)
}

/// The sync plan for one item: what changed since the last push decides the call.
#[derive(Debug, PartialEq)]
enum Step { Add, Update(String), Remove(String), Keep }
fn plan(prev: &Value, cal: &str, sig: &str, live: bool) -> Step {
    let ev = prev["ev"].as_str().unwrap_or_default().to_string();
    match (ev.is_empty(), live) {
        (true, true) => Step::Add,
        (true, false) => Step::Keep,
        (false, false) => Step::Remove(ev),
        (false, true) if prev["cal"].as_str() != Some(cal) => Step::Remove(ev), // moved calendar: removed now, added next pass
        (false, true) if prev["sig"].as_str() == Some(sig) => Step::Keep,
        (false, true) => Step::Update(ev),
    }
}

/// Keeps team calendars in step with the workspaces: items are {id, workspace, title, due: YYYY-MM-DD, done}. Open
/// dated items in a workspace with a calendar become all-day events; changed ones are updated; done, undated or
/// vanished ones are removed. What was pushed is remembered (settings `gcal.pushed`), so unchanged items cost nothing.
#[tauri::command(async)]
pub fn gcal_sync_team(st: State<AppState>, items: Vec<Value>) -> R<Value> {
    let mut c = cfg(&st)?;
    let team = c["team"].clone();
    let mut pushed = c["pushed"].as_object().cloned().unwrap_or_default();
    let auth = format!("Bearer {}", token(&st)?);
    let (mut added, mut updated, mut removed) = (0, 0, 0);
    let mut seen = std::collections::HashSet::new();
    let mut todo: Vec<(String, String, String, Option<Value>)> = Vec::new(); // (id, calendar, sig, body if live)
    for it in items.iter().take(2000) {
        let (id, title, due) = (s(it, "id"), s(it, "title"), s(it, "due"));
        let cal = team[s(it, "workspace")].as_str().unwrap_or_default().to_string();
        let date = chrono::NaiveDate::parse_from_str(&due, "%Y-%m-%d").ok();
        if id.is_empty() { continue; }
        seen.insert(id.clone());
        let live = !cal.is_empty() && !title.is_empty() && date.is_some() && !it["done"].as_bool().unwrap_or(false);
        let body = date.filter(|_| live).map(|d| json!({ "summary": title.chars().take(300).collect::<String>(), "start": { "date": due },
            "end": { "date": (d + chrono::Duration::days(1)).to_string() }, "extendedProperties": { "private": { "ptcc": id } } }));
        todo.push((id, cal, format!("{title}|{due}"), body));
    }
    for (id, prev) in pushed.clone() {
        if !seen.contains(&id) { todo.push((id.clone(), prev["cal"].as_str().unwrap_or_default().into(), String::new(), None)); }
    }
    let mut fail = None;
    for (id, cal, sig, body) in todo {
        let prev = pushed.get(&id).cloned().unwrap_or(Value::Null);
        let r = match plan(&prev, &cal, &sig, body.is_some()) {
            Step::Keep => Ok(()),
            Step::Add => agent().post(&events(&cal)).set("Authorization", &auth).send_json(body.unwrap()).map_err(gerr)
                .and_then(|r| r.into_json::<Value>().map_err(|e| e.to_string()))
                .map(|v| { pushed.insert(id.clone(), json!({ "cal": cal, "sig": sig, "ev": v["id"] })); added += 1; }),
            Step::Update(ev) => agent().request("PATCH", &format!("{}/{}", events(&cal), enc(&ev))).set("Authorization", &auth).send_json(body.unwrap()).map_err(gerr)
                .map(|_| { pushed.insert(id.clone(), json!({ "cal": cal, "sig": sig, "ev": ev })); updated += 1; }),
            Step::Remove(ev) => {
                let pc = prev["cal"].as_str().unwrap_or_default().to_string();
                match agent().delete(&format!("{}/{}", events(&pc), enc(&ev))).set("Authorization", &auth).call() {
                    Ok(_) | Err(ureq::Error::Status(404 | 410, _)) => { pushed.remove(&id); removed += 1; Ok(()) }
                    Err(x) => Err(gerr(x)),
                }
            }
        };
        if let Err(e) = r { fail = Some(e); break; } // keep what got done; the next pass retries the rest
    }
    c["pushed"] = Value::Object(pushed);
    save_cfg(&st, &c)?;
    match fail {
        Some(e) => Err(e),
        None => Ok(json!({ "added": added, "updated": updated, "removed": removed })),
    }
}

/// A calendar's events URL; the id is percent-encoded (group calendar ids contain '@' and '#').
fn events(cal: &str) -> String {
    let id = if cal.is_empty() { "primary" } else { cal };
    let enc: String = id.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    format!("https://www.googleapis.com/calendar/v3/calendars/{enc}/events")
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636_example() {
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn callback_needs_matching_state() {
        assert_eq!(code_from("/?state=abc&code=4%2F0Ab", "abc").unwrap(), "4/0Ab");
        assert!(code_from("/?state=evil&code=x", "abc").is_err());
        assert!(code_from("/?error=access_denied&state=abc", "abc").unwrap_err().contains("cancelled"));
    }

    #[test]
    fn team_sync_plans_each_item_from_what_was_pushed() {
        let none = Value::Null;
        let was = json!({ "cal": "team@group", "sig": "Logo|2026-11-12", "ev": "e1" });
        assert_eq!(plan(&none, "team@group", "Logo|2026-11-12", true), Step::Add);
        assert_eq!(plan(&none, "", "", false), Step::Keep, "never pushed, nothing to remove");
        assert_eq!(plan(&was, "team@group", "Logo|2026-11-12", true), Step::Keep, "unchanged costs nothing");
        assert_eq!(plan(&was, "team@group", "Logo|2026-11-14", true), Step::Update("e1".into()));
        assert_eq!(plan(&was, "team@group", "Logo|2026-11-12", false), Step::Remove("e1".into()), "done or deleted");
        assert_eq!(plan(&was, "other@group", "Logo|2026-11-12", true), Step::Remove("e1".into()), "moved calendar");
        assert_eq!(events("abc#x@group.calendar.google.com"), "https://www.googleapis.com/calendar/v3/calendars/abc%23x%40group.calendar.google.com/events");
        assert!(events("").ends_with("/primary/events"));
    }

    #[test]
    fn event_times_are_checked() {
        assert_eq!(when(&json!({ "date": "2026-11-10" })).unwrap(), json!({ "date": "2026-11-10" }));
        assert!(when(&json!({ "dateTime": "2026-11-10T09:00:00-06:00" })).is_ok());
        assert!(when(&json!({ "date": "tomorrow" })).is_err());
        assert_eq!(enc("abc_12-x/../y"), "abc_12-xy");
    }
}
