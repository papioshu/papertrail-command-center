//! Read-only parsers for the four continuity files. Tolerant: the formats differ between projects.
//! Every extracted row keeps its source file + line for drill-down.

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub section: String,
    pub status: &'static str,
    pub text: String,
    pub author: Option<&'static str>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub date: Option<String>,
    pub title: String,
    pub detail: String,
    pub author: Option<&'static str>,
    pub code: Option<String>,
    pub line: usize,
}

pub fn author_of(s: &str) -> Option<&'static str> {
    if s.contains("🟦") || s.contains("CLAUDE") {
        Some("claude")
    } else if s.contains("🟧") || s.contains("CODEX") || s.contains("ASTRA") {
        Some("codex")
    } else if s.contains("Owner") || s.contains("owner —") {
        Some("owner")
    } else {
        None
    }
}

fn first_date(s: &str) -> Option<String> {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(9)).find_map(|i| {
        let w = &s.get(i..i + 10)?;
        let ok = w.bytes().enumerate().all(|(j, c)| if j == 4 || j == 7 { c == b'-' } else { c.is_ascii_digit() });
        (ok && w.starts_with("20")).then(|| w.to_string())
    })
}

fn section_status(section: &str) -> &'static str {
    let s = section.to_uppercase().replace("NON-BLOCKING", "").replace("NON BLOCKING", "");
    if s.contains("BLOCK") {
        "blocked"
    } else if s.contains("DONE") || s.contains("COMPLETE") {
        "done"
    } else if s.contains("LATER") || s.contains("BACKLOG") || s.contains("DEFER") {
        "deferred"
    } else if s.starts_with("NOW") || s.contains("IN PROGRESS") || s.contains("ACTIVE") {
        "active"
    } else {
        "pending"
    }
}

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Sections whose name states a status (NOW, NEXT, BLOCKED, LATER, BACKLOG, DONE...).
fn is_status_section(section: &str) -> bool {
    let s = section.to_uppercase();
    section_status(section) != "pending" || s.starts_with("NEXT") || s.starts_with("READY") || s.starts_with("TODO")
}

/// Checkbox items anywhere. Inside a status section, every top-level bullet or paragraph is an item too
/// (many TASKS files list work as plain bullets or paragraphs under NOW / NEXT / BLOCKED).
pub fn parse_tasks(md: &str) -> Vec<Task> {
    let mut out: Vec<Task> = Vec::new();
    let mut section = String::new();
    let mut in_item = false;
    let mut in_fence = false;
    for (i, raw) in md.lines().enumerate() {
        let t = raw.trim_start();
        if t.starts_with("```") {
            in_fence = !in_fence;
            in_item = false;
            continue;
        }
        if in_fence {
            continue;
        }
        if raw.starts_with("## ") || raw.starts_with("### ") {
            section = raw.trim_start_matches('#').trim().to_string();
            in_item = false;
            continue;
        }
        let checkbox = t.strip_prefix("- [ ]").map(|r| (false, r)).or_else(|| {
            t.strip_prefix("- [x]").or_else(|| t.strip_prefix("- [X]")).map(|r| (true, r))
        });
        let status_sec = is_status_section(&section);
        let top_level = !raw.starts_with(' ') && !raw.starts_with('\t');
        let plain = if status_sec && top_level && checkbox.is_none() {
            if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                Some(r)
            } else if !in_item && !t.is_empty() && !t.starts_with(['#', '>', '|', '<', '-']) {
                Some(t) // paragraph item
            } else {
                None
            }
        } else {
            None
        };
        if let Some((done, rest)) = checkbox.or(plain.map(|r| (false, r))) {
            let status = if done { "done" } else { section_status(&section) };
            out.push(Task { section: section.clone(), status, text: clean(rest), author: author_of(rest), line: i + 1 });
            in_item = true;
        } else if in_item && !t.is_empty() && !(top_level && t.starts_with("- ")) {
            // Continuation of the previous item (wrapped line or nested bullet).
            let last = out.last_mut().unwrap();
            if last.text.len() < 1200 {
                last.text = format!("{} {}", last.text, clean(t));
            }
        } else if t.is_empty() {
            in_item = false;
        }
    }
    out
}

/// Dated headings (PROJECT LOG) and D### / dated headings (DECISIONS).
pub fn parse_entries(md: &str, decisions: bool) -> Vec<Entry> {
    let lines: Vec<&str> = md.lines().collect();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let lvl = l.chars().take_while(|c| *c == '#').count();
        if !(2..=4).contains(&lvl) {
            continue;
        }
        let head = l[lvl..].trim();
        let code = head
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .filter(|w| w.len() >= 2 && w.starts_with('D') && w[1..].chars().all(|c| c.is_ascii_digit()))
            .map(str::to_string);
        let mut date = first_date(head);
        if decisions && code.is_none() && date.is_none() {
            continue;
        }
        if !decisions && date.is_none() {
            continue;
        }
        // Body: until the next heading of the same or higher level.
        let mut body = Vec::new();
        for b in &lines[i + 1..] {
            let bl = b.chars().take_while(|c| *c == '#').count();
            if bl > 0 && bl <= lvl {
                break;
            }
            body.push(*b);
        }
        let body = body.join("\n");
        if date.is_none() {
            date = body.lines().take(6).find_map(first_date);
        }
        out.push(Entry {
            date,
            title: clean(head),
            detail: crate::util::truncate(&clean(&body), 600),
            author: author_of(head),
            code,
            line: i + 1,
        });
    }
    out
}

/// (summary, next action) from a CONTINUITY LOG.
pub fn parse_continuity(md: &str) -> (Option<String>, Option<String>) {
    // Skip past the annotation block's closing `-->`.
    let body = match md.find("continuity-annotation:end") {
        Some(i) => md[i..].find("-->").map(|j| &md[i + j + 3..]).unwrap_or(&md[i..]),
        None => md,
    };
    let paras: Vec<String> = body.split("\n\n").map(|p| p.trim().to_string()).collect();
    let summary = paras
        .iter()
        .find(|p| p.starts_with("**As of"))
        .or_else(|| {
            paras.iter().find(|p| {
                !p.is_empty() && !p.starts_with('#') && !p.starts_with("<!--") && !p.starts_with('>')
                    && !p.starts_with("---") && !p.starts_with("Concise")
            })
        })
        .map(|p| crate::util::truncate(&clean(p), 700));
    let next = paras
        .iter()
        .find(|p| p.starts_with("**Next logical"))
        .map(|p| crate::util::truncate(&clean(p), 700));
    (summary, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tasks() {
        let md = "# T\n## NOW\n- [ ] 🟦 CLAUDE — do a\n      and more\n- [x] done b\n## BLOCKED\n- [ ] Owner — c\n## LATER (P2)\n- [ ] d\n## Phase 3 — combat\n- [ ] e\n";
        let t = parse_tasks(md);
        assert_eq!(t.len(), 5);
        assert_eq!(section_status("Operator questions (non-blocking)"), "pending");
        assert_eq!(t[0].status, "active");
        assert_eq!(t[0].text, "🟦 CLAUDE — do a and more");
        assert_eq!(t[0].author, Some("claude"));
        assert_eq!(t[1].status, "done");
        assert_eq!(t[2].status, "blocked");
        assert_eq!(t[3].status, "deferred");
        assert_eq!(t[4].status, "pending");
        let md2 = "# T
- **Status:** legend
## NOW
🟦 **CLAUDE:** saved for restart,
next merge.

- **Fix build** in
  `a.tsx`
  - nested note
- second
## Labels
- not a task
";
        let t2 = parse_tasks(md2);
        assert_eq!(t2.len(), 3, "{t2:?}");
        assert_eq!(t2[0].text, "🟦 **CLAUDE:** saved for restart, next merge.");
        assert_eq!(t2[1].text, "**Fix build** in `a.tsx` - nested note");
        assert_eq!(t2[2].status, "active");
    }
    #[test]
    fn entries() {
        let md = "# D\n## D012 🟦 CLAUDE — Pick X\nbody 2026-10-01\n## D013 — Y\n### 2026-08-11 — Vault access\ntext\n## Notes\n";
        let e = parse_entries(md, true);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].code.as_deref(), Some("D012"));
        assert_eq!(e[0].date.as_deref(), Some("2026-10-01"));
        assert_eq!(e[2].date.as_deref(), Some("2026-08-11"));
        let log = parse_entries("## 2026-10-20 🟦 CLAUDE — Session 1\nx\n## Current State\n", false);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].author, Some("claude"));
    }
    #[test]
    fn continuity() {
        let md = "<!-- continuity-annotation:end -->\n\nConcise, boilerplate.\n\n**As of 2026-11-01: works.**\n\n**Next logical action:** merge.\n";
        let (s, n) = parse_continuity(md);
        assert_eq!(s.as_deref(), Some("**As of 2026-11-01: works.**"));
        assert_eq!(n.as_deref(), Some("**Next logical action:** merge."));
    }
}
