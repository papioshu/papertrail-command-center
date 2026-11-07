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
