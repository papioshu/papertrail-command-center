//! Git inspection via the installed `git` binary: bounded, no console, read-only commands only.
use crate::util::run;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Default, Clone)]
pub struct Status {
    pub branch: Option<String>,
    pub ahead: i64,
    pub behind: i64,
    pub dirty: i64,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub sha: String,
    pub ts: String,
    pub author: String,
    pub subject: String,
    pub agent: Option<&'static str>,
    pub files: Vec<(String, i64, i64)>,
}

const T: Duration = Duration::from_secs(20);

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    // Repos are untrusted input: their own config must not run programs during read-only commands.
    // No safe.directory override (git's ownership check stays on); fsmonitor/hooks/external diff/textconv disabled.
    let mut a = vec![
        "-c", "core.quotepath=off", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=NUL",
        "-c", "diff.external=", "-c", "core.untrackedCache=false", "-c", "core.pager=cat", "-c", "log.showSignature=false",
    ];
    a.extend_from_slice(args);
    let o = run("git", &a, Some(repo), T)?;
    if o.ok { Ok(o.stdout) } else { Err(format!("git {} failed", args.first().unwrap_or(&""))) }
}

pub fn head(repo: &Path) -> Option<String> {
    git(repo, &["rev-parse", "HEAD"]).ok().map(|s| s.trim().to_string()).filter(|s| s.len() >= 7)
}

/// Ask git itself (reading config never runs programs) for repo-scoped keys, following includes and
/// worktree config. Fail closed: if git can't answer, treat the repo as risky.
pub fn risky_config(repo: &Path) -> Option<String> {
    let mut keys = match git(repo, &["config", "--local", "--includes", "--name-only", "--list"]) {
        Ok(out) => out,
        Err(e) => return Some(format!("could not read repo config ({e})")),
    };
    // Exists only when extensions.worktreeConfig is on; an error just means there is none.
    if let Ok(out) = git(repo, &["config", "--worktree", "--includes", "--name-only", "--list"]) {
        keys.push_str(&out);
    }
    keys.lines()
        .map(|k| k.trim().to_lowercase())
        // -c overrides already neutralise fsmonitor/hooks/ext-diff/pager; these are the program-valued keys that can't be
        // reliably overridden per name, so their presence fails closed.
        .find(|k| {
            k.starts_with("filter.") || k.starts_with("include.") || k.starts_with("includeif.") || k.starts_with("credential.")
                || ["core.sshcommand", "core.askpass", "core.editor", "core.alternaterefscommand"].contains(&k.as_str())
                || (k.starts_with("gpg.") && k.ends_with("program"))
                || (k.starts_with("diff.") && (k.ends_with(".command") || k.ends_with(".textconv")))
        })
        .map(|k| format!("repo config defines {k}"))
}

/// Callers must check `risky_config` first (indexer does, once per repo, before any git command).
pub fn status(repo: &Path) -> Result<Status, String> {
    parse_status(&git(repo, &["status", "--porcelain=v1", "-b"])?)
}

pub fn parse_status(out: &str) -> Result<Status, String> {
    let mut st = Status::default();
    for (i, l) in out.lines().enumerate() {
        if i == 0 && l.starts_with("## ") {
            let h = l[3..].trim_start_matches("No commits yet on ");
            let name = h.split("...").next().unwrap_or(h).split(' ').next().unwrap_or("");
            st.branch = Some(name.to_string());
            if let Some(b) = h.find('[') {
                for part in h[b + 1..].trim_end_matches(']').split(", ") {
                    if let Some(n) = part.strip_prefix("ahead ") {
                        st.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix("behind ") {
                        st.behind = n.parse().unwrap_or(0);
                    }
                }
            }
        } else if !l.trim().is_empty() {
            st.dirty += 1;
        }
    }
    Ok(st)
}

/// Commits in `range` (e.g. "abc..HEAD") or the last `limit` commits when range is None.
pub fn log(repo: &Path, range: Option<&str>, limit: usize) -> Result<Vec<Commit>, String> {
    let n = format!("-n{limit}");
    let mut args = vec![
        "log",
        n.as_str(),
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--no-show-signature",
        "--date=iso-strict",
        "--pretty=format:%x1e%H%x1f%aI%x1f%an <%ae>%x1f%s%x1f%(trailers:key=Co-Authored-By,valueonly,separator=%x1d)",
        "--numstat",
    ];
    if let Some(r) = range {
        args.push(r);
    }
    Ok(parse_log(&git(repo, &args)?))
}

pub fn parse_log(out: &str) -> Vec<Commit> {
    out.split('\x1e')
        .filter(|c| !c.trim().is_empty())
        .filter_map(|chunk| {
            let mut lines = chunk.lines();
            let head: Vec<&str> = lines.next()?.split('\x1f').collect();
            if head.len() < 4 {
                return None;
            }
            let trailers = head.get(4).copied().unwrap_or("");
            let who = format!("{} {}", head[2], trailers).to_lowercase();
            // Attribution only from recorded evidence: author identity or Co-Authored-By trailer.
            let agent = if who.contains("claude") || who.contains("noreply@anthropic.com") {
                Some("claude")
            } else if who.contains("codex") || who.contains("openai") {
                Some("codex")
            } else {
                None
            };
            let files = lines
                .filter_map(|l| {
                    let mut p = l.splitn(3, '\t');
                    let a = p.next()?;
                    let d = p.next()?;
                    let path = p.next()?;
                    Some((path.to_string(), a.parse().unwrap_or(0), d.parse().unwrap_or(0)))
                })
                .collect();
            Some(Commit {
                sha: head[0].to_string(),
                ts: head[1].to_string(),
                author: head[2].to_string(),
                subject: head[3].to_string(),
                agent,
                files,
            })
        })
        .collect()
}

/// Workstream bucket for a changed path: the first meaningful directory.
pub fn workstream(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    let container = ["src", "app", "lib", "packages", "apps", "crates", "source", "game"];
    if segs.len() < 2 {
        return "root".into();
    }
    let mut i = 0;
    while i + 2 < segs.len() && container.contains(&segs[i]) {
        i += 1;
    }
    segs[i].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_and_log() {
        let s = parse_status("## main...origin/main [ahead 2, behind 1]\n M a\n?? b\n").unwrap();
        assert_eq!((s.branch.as_deref(), s.ahead, s.behind, s.dirty), (Some("main"), 2, 1, 2));
        assert_eq!(parse_status("## No commits yet on master
").unwrap().branch.as_deref(), Some("master"));
        let d = std::env::temp_dir().join(format!("ptcc-risky-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(risky_config(&d).is_some(), "not a repo: fail closed");
        crate::util::run("git", &["init", "-q"], Some(&d), T).unwrap();
        assert!(risky_config(&d).is_none());
        // BOM + odd spacing a hand parser would miss; git still reports the key.
        let cfg = d.join(".git").join("config");
        let text = format!("\u{feff}{}\n  [ filter \"x\" ]\n\tclean = evil\n", std::fs::read_to_string(&cfg).unwrap());
        std::fs::write(&cfg, text).unwrap();
        assert!(risky_config(&d).is_some(), "filter (or unreadable config) must be flagged");
        let _ = std::fs::remove_dir_all(d);
        let out = "\x1eabc\x1f2026-11-01T10:00:00-05:00\x1fMe <m@x>\x1fFix\x1fClaude Opus <noreply@anthropic.com>\n3\t1\tsrc/game/board/a.ts\n-\t-\tassets/x.png\n\x1edef\x1f2026-11-01T09:00:00-05:00\x1fMe <m@x>\x1fInit\x1f\n";
        let c = parse_log(out);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].agent, Some("claude"));
        assert_eq!(c[0].files.len(), 2);
        assert_eq!(c[1].agent, None);
        assert_eq!(workstream("src/game/board/a.ts"), "board");
        assert_eq!(workstream("src/ui/x.ts"), "ui");
        assert_eq!(workstream("README.md"), "root");
        assert_eq!(workstream("docs/a.md"), "docs");
    }
}
