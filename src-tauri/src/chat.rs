//! CHAT: read Claude Code / Codex transcripts as a conversation. Read-only; never loads a whole file.
use serde::Serialize;
use serde_json::Value;

pub const TEXT_CAP: usize = 200_000;
pub const IMG_CAP: usize = 3_000_000;

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct Img {
    pub media_type: String,
    pub data: Option<String>,
    pub omitted: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct ChatEntry {
    pub off: u64,
    pub ts: String,
    /// you | agent | reasoning | system | tool_use | tool_result
    pub kind: String,
    pub text: String,
    pub tool_id: Option<String>,
    pub name: Option<String>,
    pub input: Option<Value>,
    pub is_error: bool,
    pub images: Vec<Img>,
    pub truncated: bool,
}

struct Ctx {
    off: u64,
    ts: String,
    full: bool,
}

impl Ctx {
    fn entry(&self, kind: &str, text: &str) -> ChatEntry {
        let mut e = ChatEntry { off: self.off, ts: self.ts.clone(), kind: kind.into(), ..Default::default() };
        e.text = self.cap(text, &mut e.truncated);
        e
    }
    fn cap(&self, s: &str, truncated: &mut bool) -> String {
        if self.full || s.len() <= TEXT_CAP || s.chars().count() <= TEXT_CAP {
            return s.to_string();
        }
        *truncated = true;
        s.chars().take(TEXT_CAP).collect()
    }
    /// Caps every string inside a tool input.
    fn cap_value(&self, v: &Value, truncated: &mut bool) -> Value {
        match v {
            Value::String(s) => Value::String(self.cap(s, truncated)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.cap_value(x, truncated)).collect()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), self.cap_value(x, truncated))).collect()),
            x => x.clone(),
        }
    }
    fn img(&self, media_type: &str, data: Option<&str>) -> Img {
        let omitted = !self.full && data.map_or(false, |d| d.len() > IMG_CAP);
        Img { media_type: media_type.into(), data: if omitted { None } else { data.map(String::from) }, omitted }
    }
}

fn str_of(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// One transcript line → zero or more entries. `full` disables the size caps.
pub fn normalize(agent: &str, line: &Value, off: u64, full: bool) -> Vec<ChatEntry> {
    let cx = Ctx { off, ts: str_of(&line["timestamp"]).to_string(), full };
    if agent == "codex" { codex(&cx, line) } else { claude(&cx, line) }
}

fn you_or_system(cx: &Ctx, text: &str, meta: bool) -> ChatEntry {
    let kind = if meta || !crate::agents::is_real_prompt(text) { "system" } else { "you" };
    cx.entry(kind, text)
}

fn claude(cx: &Ctx, v: &Value) -> Vec<ChatEntry> {
    let ty = str_of(&v["type"]);
    if ty != "user" && ty != "assistant" {
        return vec![];
    }
    let meta = v["isMeta"].as_bool() == Some(true);
    let side = v["isSidechain"].as_bool() == Some(true);
    let mut out = vec![];
    match &v["message"]["content"] {
        Value::String(s) if ty == "user" => out.push(you_or_system(cx, s, meta)),
        Value::String(s) => out.push(cx.entry("agent", s)),
        Value::Array(items) => {
            for b in items {
                match str_of(&b["type"]) {
                    "text" if ty == "user" => out.push(you_or_system(cx, str_of(&b["text"]), meta)),
                    "text" => out.push(cx.entry("agent", str_of(&b["text"]))),
                    "thinking" if !str_of(&b["thinking"]).is_empty() => out.push(cx.entry("reasoning", str_of(&b["thinking"]))),
                    "tool_use" => {
                        let mut e = cx.entry("tool_use", "");
                        e.tool_id = b["id"].as_str().map(String::from);
                        e.name = b["name"].as_str().map(String::from);
                        e.input = Some(cx.cap_value(&b["input"], &mut e.truncated));
                        out.push(e);
                    }
                    "tool_result" => {
                        let (text, imgs) = match &b["content"] {
                            Value::String(s) => (s.clone(), vec![]),
                            Value::Array(a) => {
                                let t = a.iter().filter(|x| x["type"] == "text").map(|x| str_of(&x["text"])).collect::<Vec<_>>().join("\n");
                                let i = a
                                    .iter()
                                    .filter(|x| x["type"] == "image")
                                    .map(|x| cx.img(str_of(&x["source"]["media_type"]), x["source"]["data"].as_str()))
                                    .collect();
                                (t, i)
                            }
                            _ => (String::new(), vec![]),
                        };
                        let mut e = cx.entry("tool_result", &text);
                        e.tool_id = b["tool_use_id"].as_str().map(String::from);
                        e.is_error = b["is_error"].as_bool() == Some(true);
                        e.images = imgs;
                        out.push(e);
                    }
                    "image" => {
                        let mut e = cx.entry(if ty == "user" && !meta { "you" } else { "system" }, "");
                        e.images.push(cx.img(str_of(&b["source"]["media_type"]), b["source"]["data"].as_str()));
                        out.push(e);
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if side {
        for e in &mut out {
            e.kind = "system".into();
            e.text = format!("[subagent] {}", e.text);
        }
    }
    out
}

fn joined_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn codex(cx: &Ctx, v: &Value) -> Vec<ChatEntry> {
    if v["type"] != "response_item" {
        return vec![];
    }
    let p = &v["payload"];
    let mut e = match str_of(&p["type"]) {
        "message" => {
            let text = joined_text(&p["content"]);
            let kind = match str_of(&p["role"]) {
                "user" if !text.trim_start().starts_with('<') => "you",
                "assistant" => "agent",
                _ => "system",
            };
            cx.entry(kind, &text)
        }
        "function_call" | "custom_tool_call" => {
            let mut e = cx.entry("tool_use", "");
            e.name = p["name"].as_str().map(String::from);
            let raw = if p["type"] == "function_call" {
                let a = str_of(&p["arguments"]);
                serde_json::from_str(a).unwrap_or_else(|_| Value::String(a.into()))
            } else {
                serde_json::json!({ "input": str_of(&p["input"]) })
            };
            e.input = Some(cx.cap_value(&raw, &mut e.truncated));
            e
        }
        "function_call_output" | "custom_tool_call_output" => cx.entry("tool_result", &joined_text(&p["output"])),
        "reasoning" => {
            let t = joined_text(&p["summary"]);
            if t.trim().is_empty() {
                return vec![];
            }
            cx.entry("reasoning", &t)
        }
        _ => return vec![],
    };
    if e.tool_id.is_none() {
        e.tool_id = p["call_id"].as_str().map(String::from);
    }
    vec![e]
}

/* ---------- reading by byte offset ---------- */

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub const PAGE_BYTES: u64 = 4 << 20;
/// Most bytes one tail call reads, so a burst of output can't balloon memory.
const TAIL_BYTES: u64 = 16 << 20;

#[derive(Serialize, Debug, Default)]
pub struct Page {
    /// Oldest first.
    pub entries: Vec<ChatEntry>,
    /// Pass as `before` for older entries; None = start of file reached.
    pub cursor: Option<u64>,
    /// Offset just after the last complete line; pass as `from` to tail.
    pub end: u64,
    /// Tail only: the file is now shorter than `from` (rewritten).
    pub reset: bool,
    pub bad_lines: usize,
}

fn read_at(f: &mut File, lo: u64, hi: u64) -> Result<Vec<u8>, String> {
    f.seek(SeekFrom::Start(lo)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; (hi - lo) as usize];
    f.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Parse one line (without its `\n`); None for a blank line.
fn parse_line(agent: &str, bytes: &[u8], off: u64, full: bool, bad: &mut usize) -> Option<Vec<ChatEntry>> {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return None;
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(v) => Some(normalize(agent, &v, off, full)),
        Err(_) => {
            *bad += 1;
            None
        }
    }
}

/// Offset just after the last `\n` (0 when the file has none).
fn last_line_end(f: &mut File, len: u64) -> Result<u64, String> {
    let mut hi = len;
    while hi > 0 {
        let lo = hi.saturating_sub(PAGE_BYTES);
        let buf = read_at(f, lo, hi)?;
        if let Some(i) = buf.iter().rposition(|&b| b == b'\n') {
            return Ok(lo + i as u64 + 1);
        }
        hi = lo;
    }
    Ok(0)
}

/// Entries ending at `before` (default: end of file), at least `limit` of them unless the file starts first.
pub fn read_page(path: &Path, agent: &str, before: Option<u64>, limit: usize) -> Result<Page, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let end = last_line_end(&mut f, len)?;
    let mut hi = before.unwrap_or(end).min(end);
    let mut page = Page { end, ..Default::default() };
    let mut lines: Vec<Vec<ChatEntry>> = vec![]; // newest first
    let mut count = 0;
    let mut win = PAGE_BYTES;
    'outer: while hi > 0 {
        let lo = hi.saturating_sub(win);
        let buf = read_at(&mut f, lo, hi)?; // ends with '\n': `hi` is always a line end
        let start = if lo == 0 {
            0
        } else {
            // The first newline before the final one marks where whole lines begin.
            match buf[..buf.len() - 1].iter().position(|&b| b == b'\n') {
                Some(i) => i + 1,
                None => {
                    win *= 2; // one line longer than the window: widen until it fits
                    continue;
                }
            }
        };
        let mut segs = vec![];
        let mut s = start;
        for (i, _) in buf.iter().enumerate().skip(start).filter(|(_, &b)| b == b'\n') {
            segs.push((s, i));
            s = i + 1;
        }
        for &(s, e) in segs.iter().rev() {
            let off = lo + s as u64;
            if let Some(es) = parse_line(agent, &buf[s..e], off, false, &mut page.bad_lines) {
                count += es.len();
                lines.push(es);
            }
            hi = off;
            if count >= limit {
                break 'outer;
            }
        }
        win = PAGE_BYTES;
    }
    page.cursor = (hi > 0).then_some(hi);
    page.entries = lines.into_iter().rev().flatten().collect();
    Ok(page)
}

/// Entries from complete lines after `from`.
pub fn read_tail(path: &Path, agent: &str, from: u64) -> Result<Page, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    if len < from {
        return Ok(Page { reset: true, end: from, ..Default::default() });
    }
    let buf = read_at(&mut f, from, len.min(from + TAIL_BYTES))?;
    let mut page = Page { end: from, ..Default::default() };
    let mut start = 0;
    for (i, _) in buf.iter().enumerate().filter(|(_, &b)| b == b'\n') {
        if let Some(es) = parse_line(agent, &buf[start..i], from + start as u64, false, &mut page.bad_lines) {
            page.entries.extend(es);
        }
        start = i + 1;
    }
    page.end = from + start as u64;
    page.cursor = None;
    Ok(page)
}

/// One line in full (no caps), for "Load full".
pub fn read_entry(path: &Path, agent: &str, off: u64) -> Result<Vec<ChatEntry>, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.seek(SeekFrom::Start(off)).map_err(|e| e.to_string())?;
    let mut line = vec![];
    BufReader::new(f).read_until(b'\n', &mut line).map_err(|e| e.to_string())?;
    let line = line.strip_suffix(b"\n").unwrap_or(&line);
    let mut bad = 0;
    parse_line(agent, line, off, true, &mut bad).ok_or_else(|| "That entry could not be read.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kinds(v: &[ChatEntry]) -> Vec<&str> {
        v.iter().map(|e| e.kind.as_str()).collect()
    }

    #[test]
    fn claude_prompt_reply_tool() {
        let p = normalize("claude", &json!({"type":"user","timestamp":"t","message":{"role":"user","content":"hello"}}), 0, false);
        assert_eq!(kinds(&p), ["you"]);
        assert_eq!(p[0].text, "hello");
        let a = normalize(
            "claude",
            &json!({"type":"assistant","timestamp":"t","message":{"content":[{"type":"text","text":"ok"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}),
            5,
            false,
        );
        assert_eq!(kinds(&a), ["agent", "tool_use"]);
        assert_eq!(a[1].tool_id.as_deref(), Some("t1"));
        assert_eq!(a[1].name.as_deref(), Some("Bash"));
        assert_eq!(a[1].input.as_ref().unwrap()["command"], "ls");
        assert_eq!(a[1].off, 5);
        let r = normalize(
            "claude",
            &json!({"type":"user","timestamp":"t","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a\nb","is_error":true}]}}),
            9,
            false,
        );
        assert_eq!(kinds(&r), ["tool_result"]);
        assert_eq!(r[0].text, "a\nb");
        assert!(r[0].is_error);
        assert_eq!(r[0].tool_id.as_deref(), Some("t1"));
    }

    #[test]
    fn claude_skips_noise() {
        assert!(normalize("claude", &json!({"type":"api-request-blob","message":{"role":"user","content":"dup"}}), 0, false).is_empty());
        let c = normalize("claude", &json!({"type":"user","message":{"content":"<command-name>/clear</command-name>"}}), 0, false);
        assert_eq!(kinds(&c), ["system"]);
        let m = normalize("claude", &json!({"type":"user","isMeta":true,"message":{"content":"Caveat: hi"}}), 0, false);
        assert_eq!(kinds(&m), ["system"]);
        let m2 = normalize("claude", &json!({"type":"user","isMeta":true,"message":{"content":"plain"}}), 0, false);
        assert_eq!(kinds(&m2), ["system"]);
        let t = normalize("claude", &json!({"type":"assistant","message":{"content":[{"type":"thinking","thinking":""}]}}), 0, false);
        assert!(t.is_empty());
        let s = normalize("claude", &json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"sub"}]}}), 0, false);
        assert_eq!(kinds(&s), ["system"]);
        assert_eq!(s[0].text, "[subagent] sub");
    }

    #[test]
    fn codex_message_and_calls() {
        let u = normalize("codex", &json!({"type":"response_item","timestamp":"t","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}), 0, false);
        assert_eq!(kinds(&u), ["you"]);
        assert_eq!(u[0].text, "hi");
        let d = normalize("codex", &json!({"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}), 0, false);
        assert_eq!(kinds(&d), ["system"]);
        let env = normalize("codex", &json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x"}]}}), 0, false);
        assert_eq!(kinds(&env), ["system"]);
        let a = normalize("codex", &json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}), 0, false);
        assert_eq!(kinds(&a), ["agent"]);
        let f = normalize("codex", &json!({"type":"response_item","payload":{"type":"function_call","name":"exec","arguments":"{\"cmd\":\"ls\"}","call_id":"c1"}}), 0, false);
        assert_eq!(kinds(&f), ["tool_use"]);
        assert_eq!(f[0].input.as_ref().unwrap()["cmd"], "ls");
        assert_eq!(f[0].tool_id.as_deref(), Some("c1"));
        let o = normalize("codex", &json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"c1","output":[{"type":"input_text","text":"ok"}]}}), 0, false);
        assert_eq!(kinds(&o), ["tool_result"]);
        assert_eq!(o[0].text, "ok");
        let ct = normalize("codex", &json!({"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":"text(1)","call_id":"c2"}}), 0, false);
        assert_eq!(ct[0].input.as_ref().unwrap()["input"], "text(1)");
        assert!(normalize("codex", &json!({"type":"event_msg","payload":{"type":"agent_message","message":"dup"}}), 0, false).is_empty());
        assert!(normalize("codex", &json!({"type":"response_item","payload":{"type":"reasoning","summary":[]}}), 0, false).is_empty());
    }

    fn prompt(t: &str) -> String {
        json!({"type":"user","timestamp":"t","message":{"content":t}}).to_string() + "\n"
    }
    fn tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("ptcc-chat-{name}-{}.jsonl", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p
    }
    fn texts(p: &Page) -> Vec<String> {
        p.entries.iter().map(|e| e.text.clone()).collect()
    }

    #[test]
    fn page_newest_first_then_older() {
        let body: String = (0..10).map(|i| prompt(&format!("p{i}"))).collect();
        let off7 = (0..7).map(|i| prompt(&format!("p{i}")).len()).sum::<usize>() as u64;
        let path = tmp("page", body.as_bytes());
        let a = read_page(&path, "claude", None, 3).unwrap();
        assert_eq!(texts(&a), ["p7", "p8", "p9"]);
        assert_eq!(a.cursor, Some(off7));
        assert_eq!(a.end, body.len() as u64);
        let b = read_page(&path, "claude", a.cursor, 3).unwrap();
        assert_eq!(texts(&b), ["p4", "p5", "p6"]);
        let mut seen = [texts(&b), texts(&a)].concat();
        let mut cur = b.cursor;
        while let Some(c) = cur {
            let p = read_page(&path, "claude", Some(c), 3).unwrap();
            seen = [texts(&p), seen].concat();
            cur = p.cursor;
        }
        assert_eq!(seen, (0..10).map(|i| format!("p{i}")).collect::<Vec<_>>());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn page_handles_line_bigger_than_window() {
        let big = json!({"type":"assistant","message":{"content":[{"type":"text","text":"x".repeat(5 << 20)}]}}).to_string() + "\n";
        let body = prompt("a") + &big + &prompt("b");
        let path = tmp("big", body.as_bytes());
        let mut all = vec![];
        let mut cur = None;
        loop {
            let p = read_page(&path, "claude", cur, 1).unwrap();
            all = [p.entries, all].concat();
            cur = p.cursor;
            if cur.is_none() {
                break;
            }
        }
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].text, "a");
        assert!(all[1].truncated);
        assert_eq!(all[2].text, "b");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn tail_skips_partial_line() {
        let first = prompt("p0");
        let second = prompt("p1");
        let (head, rest) = second.split_at(10);
        let path = tmp("tail", (first.clone() + head).as_bytes());
        let t = read_tail(&path, "claude", 0).unwrap();
        assert_eq!(texts(&t), ["p0"]);
        assert_eq!(t.end, first.len() as u64);
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(rest.as_bytes()).unwrap();
        let t2 = read_tail(&path, "claude", t.end).unwrap();
        assert_eq!(texts(&t2), ["p1"]);
        assert!(!t2.reset);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn tail_reports_reset() {
        let body = prompt("p0");
        let path = tmp("reset", body.as_bytes());
        let t = read_tail(&path, "claude", body.len() as u64 + 100).unwrap();
        assert!(t.reset);
        assert!(t.entries.is_empty());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn bad_lines_counted() {
        let mut body = prompt("p0").into_bytes();
        body.extend_from_slice(b"{not json\n");
        body.extend_from_slice(&[0xff, 0xfe, b'\n']);
        body.extend_from_slice(prompt("p1").as_bytes());
        let path = tmp("bad", &body);
        let p = read_page(&path, "claude", None, 50).unwrap();
        assert_eq!(texts(&p), ["p0", "p1"]);
        assert_eq!(p.bad_lines, 2);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn read_entry_full() {
        let big = "A".repeat(IMG_CAP + 1);
        let line = json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"x","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":big}}]}]}}).to_string() + "\n";
        let first = prompt("p0");
        let path = tmp("entry", (first.clone() + &line).as_bytes());
        let paged = read_page(&path, "claude", None, 10).unwrap();
        assert!(paged.entries[1].images[0].omitted);
        let e = read_entry(&path, "claude", first.len() as u64).unwrap();
        assert!(e[0].images[0].data.is_some());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn caps_big_image_and_text() {
        let big = "A".repeat(IMG_CAP + 1);
        let line = json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"x","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":big}}]}]}});
        let e = normalize("claude", &line, 0, false);
        assert!(e[0].images[0].omitted);
        assert!(e[0].images[0].data.is_none());
        let full = normalize("claude", &line, 0, true);
        assert!(!full[0].images[0].omitted);
        assert!(full[0].images[0].data.is_some());
        let long = "b".repeat(TEXT_CAP + 1);
        let t = normalize("claude", &json!({"type":"assistant","message":{"content":[{"type":"text","text":long}]}}), 0, false);
        assert!(t[0].truncated);
        assert_eq!(t[0].text.chars().count(), TEXT_CAP);
        let t2 = normalize("claude", &json!({"type":"assistant","message":{"content":[{"type":"text","text":long}]}}), 0, true);
        assert!(!t2[0].truncated);
    }
}
