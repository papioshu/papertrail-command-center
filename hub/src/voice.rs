//! Ghost (owner 2026-11-10, D052): spoken commands and questions, as text in and a sentence out. The phone page
//! ("Hey Ghost"), the "Hey Siri, Ghost" shortcut and the PC all send what was heard to `POST /v1/voice`. Every read goes
//! through the change feed and every write through `op`, so Ghost can do exactly what that person could do by tapping.
use crate::api::{ok, Caller, Hub, Res};
use crate::store::random_hex;
use chrono::{Datelike, Duration, NaiveDate};
use rusqlite::Connection;
use serde_json::{json, Value};

/// What was asked, after the wake word and filler are gone.
#[derive(Debug, PartialEq)]
pub enum Intent {
    Add { title: String, due: Option<NaiveDate> },
    Done(String),
    Claim(String),
    Blocked,
    Mine,
    Due,
    Brief,
    Note(String),
    Help,
}

const HELP: &str = "Try: add task order sample tees to BOMBSQUAD due Friday. Or ask: what's blocked, what are my tasks, what's due this week, or catch me up. Say note, then anything, to save a note.";

fn clean(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_alphanumeric() || c == '\'' || c == ' ' { c.to_ascii_lowercase() } else { ' ' }).collect();
    let mut s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    for w in ["hey ghost ", "hi ghost ", "ok ghost ", "okay ghost ", "ghost ", "please ", "can you ", "could you "] {
        if let Some(r) = s.strip_prefix(w) {
            s = r.to_string();
        }
    }
    s.trim_end_matches(" please").trim().to_string()
}

/// "due friday", "due tomorrow", "due next week" at the end of a sentence (spoken dates are relative to `today`).
fn due_of(s: &str, today: NaiveDate) -> (String, Option<NaiveDate>) {
    let Some(i) = s.rfind(" due ").or_else(|| s.rfind(" by ")) else { return (s.to_string(), None) };
    let (head, tail) = (&s[..i], s[i..].trim().split_once(' ').map(|x| x.1).unwrap_or("").trim_start_matches("on ").trim());
    const DAYS: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
    let d = match tail {
        "today" | "tonight" => Some(today),
        "tomorrow" => Some(today + Duration::days(1)),
        "next week" => Some(today + Duration::days(7)),
        t => DAYS.iter().position(|d| t == *d || t == format!("next {d}")).map(|w| {
            let ahead = (w as i64 - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
            today + Duration::days(if ahead == 0 { 7 } else { ahead })
        }),
    };
    match d {
        Some(d) => (head.to_string(), Some(d)),
        None => (s.to_string(), None),
    }
}

pub fn parse(text: &str, today: NaiveDate) -> Intent {
    let s = clean(text);
    let after = |prefixes: &[&str]| prefixes.iter().find_map(|p| s.strip_prefix(p).map(|r| r.trim().to_string())).filter(|r| !r.is_empty());
    if let Some(r) = after(&["note that ", "take a note ", "note ", "remember that ", "remember "]) {
        return Intent::Note(r);
    }
    if let Some(r) = after(&["add a task to ", "add a task ", "add task to ", "add task ", "new task ", "create a task ", "create task ", "make a task "]) {
        let (title, due) = due_of(&r, today);
        return Intent::Add { title, due };
    }
    if let Some(r) = after(&["mark ", "finish ", "finished ", "complete ", "close "]) {
        return Intent::Done(r.trim_end_matches(" as done").trim_end_matches(" done").trim_end_matches(" as complete").to_string());
    }
    if let Some(r) = s.strip_suffix(" is done").or_else(|| s.strip_suffix(" is finished")).filter(|r| !r.is_empty()) {
        return Intent::Done(r.trim_start_matches("the ").to_string());
    }
    if let Some(r) = after(&["claim ", "i'll take ", "i will take ", "assign me ", "give me "]) {
        return Intent::Claim(r);
    }
    let has = |w: &[&str]| w.iter().any(|x| s.contains(x));
    if has(&["blocked", "stuck"]) {
        Intent::Blocked
    } else if has(&["my tasks", "my plate", "assigned to me", "am i working", "should i work", "my work"]) {
        Intent::Mine
    } else if has(&["due", "deadline", "this week", "coming up", "upcoming"]) {
        Intent::Due
    } else if has(&["catch me up", "what's new", "whats new", "brief", "status", "what's going on", "update me", "summary"]) {
        Intent::Brief
    } else {
        Intent::Help
    }
}

/// Lowercase letters and digits only, so "bomb squad" finds BOMBSQUAD.
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Pulls "to/in/for <workspace or project>" off the end of a task title. Returns (title, workspace id, project id).
fn place<'a>(title: &str, spaces: &'a [(String, String, Vec<(String, String)>)]) -> (String, Option<&'a str>, Option<&'a str>) {
    for sep in [" to ", " in ", " for ", " on "] {
        let Some(i) = title.rfind(sep) else { continue };
        let name = squash(&title[i + sep.len()..].trim_start_matches("the "));
        let head = title[..i].to_string();
        for (ws, wname, projects) in spaces {
            if !name.is_empty() && squash(wname) == name {
                return (head, Some(ws), None);
            }
            if let Some((p, _)) = projects.iter().find(|(_, pn)| !name.is_empty() && squash(pn) == name) {
                return (head, Some(ws), Some(p));
            }
        }
    }
    (title.to_string(), None, None)
}

/// The open task whose title shares the most words with what was said (ties: the first; nothing shared: none).
fn best<'a>(said: &str, tasks: &'a [Value]) -> Option<&'a Value> {
    let words: Vec<String> = said.split_whitespace().map(squash).filter(|w| w.len() > 2).collect();
    let score = |t: &Value| {
        let title = squash(t["fields"]["title"].as_str().unwrap_or(""));
        words.iter().filter(|w| title.contains(w.as_str())).count()
    };
    tasks.iter().filter(|t| t["fields"]["status"] != "done").max_by_key(|t| (score(t), std::cmp::Reverse(t["seq"].as_i64().unwrap_or(0)))).filter(|t| score(t) > 0)
}

fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        _ => format!("{}, and {}", items[..items.len() - 1].join(", "), items[items.len() - 1]),
    }
}

fn op_id() -> Result<String, crate::api::Reply> {
    let h = random_hex(16).map_err(crate::api::internal)?;
    Ok(format!("{}-{}-4{}-8{}-{}", &h[..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32]))
}

impl Hub {
    /// `POST /v1/voice {text, workspace?}` → `{say, intent}`.
    pub(crate) fn voice(&self, c: &mut Connection, who: &Caller, b: &Value) -> Res {
        let text = b["text"].as_str().unwrap_or("").chars().take(500).collect::<String>();
        let today = chrono::Local::now().date_naive();
        let intent = parse(&text, today);
        let say = |s: String| Ok(ok(json!({ "say": s, "intent": format!("{intent:?}").split(['(', ' ']).next().unwrap_or("") })));
        // Everything this person can see, per workspace (through the same rules as the app).
        let me = self.me(c, who)?.body;
        let mut spaces = Vec::new();
        let mut tasks: Vec<Value> = Vec::new();
        for w in me["workspaces"].as_array().into_iter().flatten() {
            let ws = w["id"].as_str().unwrap_or_default().to_string();
            let mut since = 0;
            let mut projects = Vec::new();
            loop {
                let Ok(r) = self.changes(c, who, &ws, &format!("since={since}&limit=500")) else { break };
                projects = r.body["projects"].as_array().cloned().unwrap_or_default();
                for x in r.body["changes"].as_array().into_iter().flatten() {
                    if x["kind"] == "task" && x["deleted"] != true {
                        let mut t = x.clone();
                        t["ws"] = json!(ws);
                        t["ws_name"] = w["name"].clone();
                        tasks.retain(|o| o["id"] != t["id"]);
                        tasks.push(t);
                    }
                }
                since = r.body["next"].as_i64().unwrap_or(since);
                if r.body["more"] != true {
                    break;
                }
            }
            let ps = projects.iter().map(|p| (p["id"].as_str().unwrap_or_default().to_string(), p["name"].as_str().unwrap_or_default().to_string())).collect();
            spaces.push((ws, w["name"].as_str().unwrap_or_default().to_string(), ps));
        }
        if spaces.is_empty() {
            return say("You're not in any workspace yet.".into());
        }
        let open: Vec<&Value> = tasks.iter().filter(|t| t["fields"]["status"] != "done").collect();
        let title = |t: &Value| t["fields"]["title"].as_str().unwrap_or("a task").to_string();
        let default_ws = b["workspace"].as_str().filter(|w| spaces.iter().any(|s| s.0 == *w)).unwrap_or(&spaces[0].0).to_string();
        match &intent {
            Intent::Help => say(if text.trim().is_empty() { HELP.into() } else { format!("I heard: {}. {HELP}", text.trim()) }),
            Intent::Add { .. } | Intent::Note(_) => {
                let (t, due, note) = match &intent {
                    Intent::Add { title, due } => (title.as_str(), *due, false),
                    Intent::Note(t) => (t.as_str(), None, true),
                    _ => return say(HELP.into()),
                };
                let (t, ws, project) = place(t, &spaces);
                let ws = ws.map(str::to_string).unwrap_or(default_ws);
                let Some(space) = spaces.iter().find(|s| s.0 == ws) else { return say(HELP.into()) };
                let Some(project) = project.map(str::to_string).or_else(|| space.2.first().map(|p| p.0.clone())) else {
                    return say(format!("{} has no shared projects yet.", space.1));
                };
                let mut t = t.trim().to_string();
                if let Some(f) = t.get(..1) {
                    t = f.to_uppercase() + &t[1..];
                }
                if t.is_empty() {
                    return say("What should the task say?".into());
                }
                let mut fields = json!({ "title": if note { format!("Note: {t}") } else { t.clone() }.chars().take(200).collect::<String>(), "status": "todo" });
                if note {
                    fields["notes"] = json!(format!("Voice note: {t}").chars().take(4000).collect::<String>());
                }
                if let Some(d) = due {
                    fields["due"] = json!(d.to_string());
                }
                let r = self.op(c, who, &ws, &json!({ "op_id": op_id()?, "action": "create", "kind": "task", "record_id": op_id()?, "project_id": project, "base_version": 0, "fields": fields }));
                let pname = space.2.iter().find(|p| p.0 == project).map(|p| p.1.as_str()).unwrap_or("");
                match r {
                    Ok(_) if note => say(format!("Saved your note to {} {pname}.", space.1)),
                    Ok(_) => say(format!("Added {t} to {} {pname}{}.", space.1, due.map(|d| format!(", due {}", d.format("%A %B %-d"))).unwrap_or_default())),
                    Err(e) => say(refused(&e)),
                }
            }
            Intent::Done(said) | Intent::Claim(said) => {
                let all: Vec<Value> = open.iter().map(|t| (*t).clone()).collect();
                let Some(t) = best(said, &all) else { return say(format!("I couldn't find an open task like {said}.")) };
                let done = matches!(intent, Intent::Done(_));
                let fields = if done { json!({ "status": "done" }) } else { json!({ "assignee": who.user }) };
                let r = self.op(c, who, t["ws"].as_str().unwrap_or(""), &json!({ "op_id": op_id()?, "action": "update", "kind": "task", "record_id": t["id"], "project_id": t["project_id"], "base_version": t["version"], "fields": fields }));
                match r {
                    Ok(x) if x.code == 200 => say(if done { format!("Marked {} done.", title(t)) } else { format!("{} is yours.", title(t)) }),
                    Ok(_) => say(format!("Someone changed {} just now. Try again.", title(t))),
                    Err(e) => say(refused(&e)),
                }
            }
            Intent::Blocked => {
                let b: Vec<String> = open.iter().filter(|t| t["fields"]["status"] == "blocked").map(|t| format!("{} in {}", title(t), t["ws_name"].as_str().unwrap_or(""))).collect();
                say(if b.is_empty() { "Nothing is blocked.".into() } else { format!("{} blocked: {}.", b.len(), list(&b[..b.len().min(5)])) })
            }
            Intent::Mine => {
                let m: Vec<String> = open.iter().filter(|t| t["fields"]["assignee"] == who.user.as_str()).map(|t| title(t)).collect();
                say(if m.is_empty() { "You have no claimed tasks. Say: claim, then a task name.".into() } else { format!("You have {}: {}.", m.len(), list(&m[..m.len().min(6)])) })
            }
            Intent::Due => {
                let end = today + Duration::days(7);
                let mut d: Vec<(NaiveDate, String)> = open
                    .iter()
                    .filter_map(|t| NaiveDate::parse_from_str(t["fields"]["due"].as_str()?.get(..10)?, "%Y-%m-%d").ok().map(|d| (d, title(t))))
                    .filter(|(d, _)| *d <= end)
                    .collect();
                d.sort();
                let late = d.iter().filter(|(x, _)| *x < today).count();
                let items: Vec<String> = d.iter().take(6).map(|(x, t)| format!("{t} {}", if *x < today { "overdue".into() } else if *x == today { "today".into() } else { x.format("%A").to_string() })).collect();
                say(if d.is_empty() { "Nothing is due in the next 7 days.".into() } else { format!("{} due{}: {}.", d.len(), if late > 0 { format!(", {late} overdue") } else { String::new() }, list(&items)) })
            }
            Intent::Brief => {
                let n = |s: &str| open.iter().filter(|t| t["fields"]["status"] == s).count();
                let soon = open.iter().filter(|t| t["fields"]["due"].as_str().and_then(|d| NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok()).is_some_and(|d| d <= today + Duration::days(7))).count();
                let recent: Vec<String> = { let mut r: Vec<&Value> = tasks.iter().collect(); r.sort_by_key(|t| std::cmp::Reverse(t["seq"].as_i64().unwrap_or(0))); r.iter().take(3).map(|t| title(t)).collect() };
                say(format!("{} open tasks: {} to do, {} in progress, {} blocked. {} due this week. Latest: {}.", open.len(), n("todo"), n("doing"), n("blocked"), soon, list(&recent)))
            }
        }
    }
}

fn refused(e: &crate::api::Reply) -> String {
    match e.body["error"].as_str() {
        Some("forbidden") => "Your role can't do that.".into(),
        Some("not_found") => "That isn't there any more.".into(),
        Some(x) => format!("That didn't work: {x}."),
        None => "That didn't work.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn understands_what_was_said() {
        let tue = d("2026-11-10");
        assert_eq!(parse("Hey Ghost, add task order sample tees to BOMBSQUAD, due Friday.", tue), Intent::Add { title: "order sample tees to bombsquad".into(), due: Some(d("2026-11-13")) });
        assert_eq!(parse("ghost add a task call the printer due tomorrow", tue), Intent::Add { title: "call the printer".into(), due: Some(d("2026-11-11")) });
        assert_eq!(parse("add task due diligence", tue), Intent::Add { title: "due diligence".into(), due: None }, "no date after 'due' keeps the words");
        assert_eq!(parse("add task ship it due tuesday", tue), Intent::Add { title: "ship it".into(), due: Some(d("2026-11-17")) }, "same weekday = next week");
        assert_eq!(parse("Mark the logo sketch as done", tue), Intent::Done("the logo sketch".into()));
        assert_eq!(parse("logo sketch is done", tue), Intent::Done("logo sketch".into()));
        assert_eq!(parse("I'll take the merch mockups", tue), Intent::Claim("the merch mockups".into()));
        assert_eq!(parse("Hey Ghost what's blocked?", tue), Intent::Blocked);
        assert_eq!(parse("what's on my plate", tue), Intent::Mine);
        assert_eq!(parse("what's due this week", tue), Intent::Due);
        assert_eq!(parse("catch me up", tue), Intent::Brief);
        assert_eq!(parse("note: the drop moves to December", tue), Intent::Note("the drop moves to december".into()));
        assert_eq!(parse("sing me a song", tue), Intent::Help);
    }

    #[test]
    fn finds_places_and_tasks() {
        let spaces = vec![("w1".to_string(), "BOMBSQUAD".to_string(), vec![("p1".to_string(), "Merch drop".to_string())]), ("w2".into(), "FLY WING".into(), vec![])];
        assert_eq!(place("order tees to bomb squad", &spaces), ("order tees".into(), Some("w1"), None));
        assert_eq!(place("print stickers for merch drop", &spaces), ("print stickers".into(), Some("w1"), Some("p1")));
        assert_eq!(place("talk to rich", &spaces), ("talk to rich".into(), None, None), "a person isn't a place");
        assert_eq!(place("mint in fly wing", &spaces).1, Some("w2"));
        let tasks = vec![json!({ "seq": 1, "fields": { "title": "Logo sketch v2", "status": "todo" } }), json!({ "seq": 2, "fields": { "title": "Order sample tees", "status": "doing" } }), json!({ "seq": 3, "fields": { "title": "Old logo", "status": "done" } })];
        assert_eq!(best("the logo sketch", &tasks).unwrap()["seq"], 1);
        assert_eq!(best("sample tees", &tasks).unwrap()["seq"], 2);
        assert!(best("old logo", &tasks).is_some_and(|t| t["seq"] == 1), "done tasks are never picked");
        assert!(best("the website", &tasks).is_none());
    }
}
