# CHAT: read and continue every agent chat in the app (design)

🟦 CLAUDE · 2026-11-06 · Status: **draft, awaiting owner approval.** Nothing has been built.
Reference: [internethokage/project-hq](https://github.com/internethokage/project-hq) at `1d72de2` (MIT). Its Chats tab
is the model for this feature; see "Borrowing from project-hq".

## Intent

The owner runs many Claude Code (and Codex) chats across projects and wants one place to:
1. **See every chat as a running log**, the whole conversation, prompts included ("i wanna be able to see the convos
   completely").
2. **Know where each chat's work stands**: in process, waiting on them, or committed and ready.
3. **Continue a chat right there in the app** ("yes right there in the app"), with **Allow / Deny cards** like
   project-hq when the agent wants to act.

Today, the Agents page lists sessions as a table, and "Continue" only resumes a project's *latest* session by
opening a terminal. Nothing in the app shows what was said.

## Goals

- A **CHAT** tab in the nav. It lists every Claude and Codex chat, newest first, with live runs on top.
- A **conversation view** for one chat: every user prompt, every agent reply, and every tool step, with nothing
  dropped. A chat that is still being written to updates live.
- A **status per chat** and **the commits it made**.
- **Reply in place** (Claude first): resume that exact session in its own folder, stream the reply into the log,
  Allow / Deny cards for gated actions, Stop, and a permission-mode switch.

## Non-goals

- Starting brand-new chats from scratch (project-hq has "New chat"). Possible later; existing launches stay as they are.
- Attachments, file previews and the `send_user_file` tool. Later, if wanted.
- Phone access. That is the parked phone-board question (D020) and is not part of this.
- Replacing the terminal. "Continue in terminal" stays as a fallback on every chat.

## Anti-goals (would count as failure even if it works)

- **Two writers on one chat.** Replying to a chat that a terminal session is actively working in would fork or
  garble it. The app's existing "transcript written in the last 3 min" check must block the reply, with an
  explicit "reply anyway".
- **An action running without the owner's say-so** in the default mode. Any timeout, crash or closed window means **deny**.
- **A slow or frozen app on big transcripts.** Some transcripts are hundreds of MB, and the PC is short on RAM. Never
  read a whole file into memory; page it.
- **Status that lies.** Every status shows the evidence behind it, as the app already does elsewhere ("A transcript
  file existing does not mean a process is running").

## The pieces

### 1. Chat list (CHAT tab)
- **Source:** the existing `sessions` table (already indexed for both agents; `path` points to the transcript).
- **Each row:** status tag (colour), agent, project, title or first prompt, last reply snippet (about 140 characters), updated time, and
  prompt/edit counts.
- **Filters:** All / Claude / Codex · project · status. Live runs (in-app or transcript written <3 min ago) are pinned on top.
- **Refresh:** on the existing `projects-updated` event, plus a 3 s tick for chats that are live.

### 2. Conversation view
- **Backend:** a new `chat_read(session_id, cursor, limit)` command that reads the transcript **by byte offset**
  in pages of about 200 entries, newest first when opened, scrolling back loads older pages, and `chat_tail(session_id, from_offset)`
  for live updates. This reuses the line-by-line JSONL reading pattern in `agents.rs` (`index_claude_file`).
- **Rendering (both agents normalised into one shape: `you` / `agent` / `tool` / `system`):**
  - **User prompts:** shown in full.
  - **Agent replies:** shown in full as markdown.
  - **Tool steps:** each run of tool steps between messages folds into one line ("4 commands · 2 edits · 1 read").
    Expanding it shows **every step in full**: the command plus its output, file edits as a diff (old/new strings),
    and reads with their paths. An "Expand all" toggle opens everything, so "completely" really means completely.
  - **Images** in a transcript (screenshots) are shown inline. **Thinking**, where the transcript has it, is a folded "Reasoning" line.
  - **System / meta entries** (hook output, command wrappers, reminders) sit behind a "Show system entries" toggle,
    off by default. They're kept rather than dropped.
- **Header:** project, folder, branch, status and its evidence, the commits list, and buttons for "Continue in terminal",
  "Open folder" and "Copy resume command".

### 3. Status and commits
States are worked out from evidence, most urgent first (owner's "in process or ready"):

| Status | Rule |
|---|---|
| **Needs you** | An in-app run has an Allow / Deny card waiting |
| **Working** | An in-app run is mid-reply, or the transcript was written in the last 3 min (labelled "in a terminal") |
| **Uncommitted changes** | Files this chat edited (`session_files`, already indexed) are dirty in `git status` of its repo |
| **Committed, not pushed** | It made commits that the branch's upstream doesn't have |
| **Ready** | It made commits, none of its files are dirty, and they're pushed (or the repo has no remote) |
| **Idle** | No file edits: a conversation only |

- **Commits per chat:** taken from its Bash steps that ran `git commit` and the `[branch abc1234] message` line in
  their output, then **checked against git** (`git cat-file -e`) so a commit that was later amended or dropped is marked "not found".
- **Dirty check:** one `git status --porcelain` per repo per refresh, cached, using the existing `gitscan.rs` approach.

### 4. Reply in place (Claude)
- **Engine:** a small Node **chat runner** (`sidecar/chat-runner.mjs`) using `@anthropic-ai/claude-agent-sdk`,
  the same library and call shape project-hq uses: `query({ prompt, options: { cwd, resume: sessionId,
  permissionMode, abortController, canUseTool, settingSources: ['user','project','local'] } })`, plus
  `interrupt()` and `setPermissionMode()`. It uses the owner's existing Claude Code login and settings; no API key.
  - **Why Node:** the SDK is the supported way to resume a session with a per-action permission callback. The app is
    Rust, so it talks to the runner over **stdin/stdout JSON lines**: no network port, and only the app can reach it.
  - **Node:** v24 is on PATH. The app checks for it and shows "Node 20+ needed for replies" if it's missing; reading
    chats doesn't need it.
  - **Packaging:** the runner has its own `sidecar/package.json`. Its `node_modules` ship as a Tauri resource, and
    the version is pinned.
  - **Folder:** resume in the session's recorded `cwd`, because Claude Code files sessions by folder. Use the same rule as
    `launcher.rs`, and refuse if the folder is gone.
  - **Lifetime:** the runner starts on the first reply and exits after 5 min with no runs, to keep RAM down.
- **Messages:**
  - app → runner: `start{run, session, cwd, prompt, mode}`, `decide{run, ask, allow, message?}`,
    `interrupt{run}`, `mode{run, mode}`.
  - runner → app: `event{run, kind, ...}` (text delta, tool step, result, cost), `ask{run, ask, tool, summary,
    input}`, `done{run, ok, error?}`.
- **Allow / Deny card:** shows the tool, a plain summary (the command, or the file plus a diff), and the full input on
  expand. The buttons are **Allow**, **Deny** (optional reason sent to Claude) and **Allow for this run**. No answer
  within 30 min means deny. If the window is hidden in the tray, a Windows notification fires ("Claude needs you · OARA");
  clicking it opens the chat.
- **Modes:** *Ask before acting* (default), *Auto-accept edits*, *Plan only*, and *Skip all prompts* (needs a confirm
  and is shown in red). The mode can be switched mid-run.
- **Composer:** Send becomes **Stop** while it runs. After the run ends, the conversation view re-reads the transcript, so the log
  stays the single source of truth.
- **Guard:** the 3-minute live check from `launcher.rs` runs before every reply.

### 5. Reply in place (Codex): later phase
`codex exec resume <id> <prompt>` can resume a session but has **no approval hook**. `codex app-server` (experimental,
v0.143 here) speaks JSON-RPC with approval requests. **Spike first:** confirm that app-server can resume an existing
thread and send approval requests. If it can, Codex gets the same cards. If not, Codex chats stay read-only, with "Continue in terminal".

## Phases (each one is usable on its own)
1. **Read:** CHAT tab, conversation view, live tail (Claude and Codex).
2. **Status and commits.**
3. **Reply for Claude:** runner, cards, Stop, modes, notification.
4. **Codex reply:** spike, then build or skip.

## Borrowing from project-hq
It's MIT-licensed. Ideas are free to use. Any code adapted from it (for example the ask/timeout flow in
`lib/agent.js`, or the transcript folding in `lib/transcript.js`) keeps its copyright notice in a header comment and
gets a NOTICE line in the README. It's read as reference only and is not added as a dependency.

## Verification
- **Unit tests (Rust):** transcript normalising for both agents (fixtures: tool runs, images, meta entries, a partial
  last line), status rules, and commit parsing (amended and missing hashes).
- **Runner:** a fake-SDK mode (project-hq ships `scripts/fake-agent-sdk.mjs` for exactly this) so the card, deny,
  timeout and stop flows are tested without spending tokens.
- **Real check over CDP on the packaged exe:**
  - open a 100 MB+ transcript and confirm it stays responsive;
  - reply to a real chat, and Allow and then Deny a command;
  - confirm the transcript shows the turn.

## Calls Claude made (owner didn't decide these; change freely)
- Codex is included for reading now; Codex replies are a later phase pending the spike.
- "Ready" means committed and pushed, or committed when the repo has no remote. It does not wait on vault logs being updated.
- Tool steps are folded by default with full detail on expand, and system entries are hidden behind a toggle.
- Node sidecar with the Agent SDK, rather than driving the Claude CLI's internal protocol from Rust.
- A 30-minute ask timeout means deny. The runner exits after 5 min idle.
- No new-chat button, attachments or phone access in this pass.

## Left to the builder
Page sizes, exact colours of the status tags, how deltas are batched for smooth streaming, and fixture contents.
