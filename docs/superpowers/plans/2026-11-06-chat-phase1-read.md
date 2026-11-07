# CHAT Phase 1 (read every chat in full) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A CHAT tab that lists every Claude Code and Codex chat and shows any one of them as a complete, live-updating conversation log.

**Architecture:** The Rust side gets a new `chat.rs`. It normalises one transcript line (Claude JSONL or Codex rollout JSONL) into flat `ChatEntry` values, and reads a transcript in byte-offset pages: backwards for history, forwards for the live tail. Three Tauri commands expose this. The React side gets `src/chat/` with two pure helpers, one that folds entries into display blocks and one minimal markdown splitter, plus a `Chat` view that renders them. Status, commits and replying come in later phases and are out of scope here.

**Tech Stack:** Tauri 2, Rust (serde_json, rusqlite, already present), React 19 + TS, vitest (already a dev dependency).

**Spec:** `docs/superpowers/specs/2026-11-06-chat-design.md` (sections 1 and 2, and the header buttons from section 2).

## Global Constraints

- **No new dependencies** in phase 1, either Rust crates or npm packages.
- **Transcripts are read-only.** Never write to, rename or lock `~/.claude/projects/**` or `~/.codex/sessions/**`.
- **Never read a whole transcript into memory.** Read at most `PAGE_BYTES = 4 MiB` per backward step. A 202 MB file (`~/.claude/projects/C--Users-User/66fb3832-88b9-4f7e-89f6-c43427f37a98.jsonl`) must open in under 2 s.
- **Per-entry caps:** text and tool input/output are capped at `TEXT_CAP = 200_000` chars (`truncated: true`). Inline images are capped at `IMG_CAP = 3_000_000` base64 chars (`omitted: true`); anything bigger is fetched on demand through `chat_entry`.
- **Only Claude lines with `type` of `user` or `assistant` are conversation.** Everything else (`api-request-blob`, `attachment`, `queue-operation`, `cost-state`, …) is skipped, because `api-request-blob` duplicates messages.
- **React text only:** never use `dangerouslySetInnerHTML` (transcripts contain arbitrary HTML).
- **Commits:** end each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **project-hq is reference only.** Never edit it and never copy files from it.

## Review Focus

1. **A partial last line still being written:** the tail must stop before it, and the next poll must return it once it's complete (Task 2 test `tail_skips_partial_line`).
2. **A transcript that was rewritten or truncated** (size < `from`): the tail returns `reset: true` and the view reloads instead of showing garbage (Task 2 test `tail_reports_reset`).
3. **A single huge line** (a pasted screenshot several MB long): it's capped and marked, not dropped, and the rest of the page still loads (Task 1 test `caps_big_image_and_text`).
4. **A session whose transcript was deleted:** the command returns `"This chat's transcript is no longer on disk."` and the view shows that message (Task 3 test `missing_file_message`).
5. **Malformed or non-UTF-8 lines:** they're skipped and counted in `bad_lines`, and the page still returns (Task 2 test `bad_lines_counted`).

---

### Task 1: Normalise transcript lines (`chat.rs`)

**Files:**
- Create: `src-tauri/src/chat.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod chat;`)

**Interfaces:**
- Produces:
  ```rust
  #[derive(Serialize, Clone, Debug, PartialEq, Default)]
  pub struct Img { pub media_type: String, pub data: Option<String>, pub omitted: bool }
  #[derive(Serialize, Clone, Debug, PartialEq, Default)]
  pub struct ChatEntry {
      pub off: u64,              // byte offset of the source line
      pub ts: String,
      pub kind: String,          // "you" | "agent" | "reasoning" | "system" | "tool_use" | "tool_result"
      pub text: String,
      pub tool_id: Option<String>,
      pub name: Option<String>,  // tool name (tool_use)
      pub input: Option<serde_json::Value>, // tool_use input (strings inside capped)
      pub is_error: bool,
      pub images: Vec<Img>,
      pub truncated: bool,
  }
  pub const TEXT_CAP: usize = 200_000;
  pub const IMG_CAP: usize = 3_000_000;
  pub fn normalize(agent: &str, line: &serde_json::Value, off: u64, full: bool) -> Vec<ChatEntry>
  ```
  `full = true` disables both caps (used by `chat_entry`).

**Mapping rules:**
- **Claude** (`agent == "claude"`):
  - Only lines with `type` `user` or `assistant` are kept.
  - `isSidechain: true` → `system`, with text prefixed `[subagent] `.
  - `user`:
    - String content → `you`, or `system` when `isMeta` is true or the text fails `agents::is_real_prompt`. Make `is_real_prompt` `pub(crate)`.
    - Array content: each `text` block follows the same you/system rule. Each `tool_result` becomes a `tool_result` entry with `tool_id = tool_use_id` and `is_error`. Its `text` is the content string, or the joined `text` items. Images go to `images`.
  - `assistant`:
    - `text` → `agent`.
    - `thinking` with non-empty `thinking` → `reasoning`; empty thinking is skipped.
    - `tool_use` → `tool_use`, with `tool_id`, `name` and `input`.
- **Codex** (`agent == "codex"`): only `type == "response_item"` lines are kept.
  - `payload.type == "message"`:
    - Role `user` → `you`, or `system` when the text starts with `<`.
    - Role `assistant` → `agent`.
    - Role `developer` → `system`.
    - Text is the joined `input_text`/`output_text` items.
  - `function_call` → `tool_use` (`name`; `input` is `arguments` parsed as JSON, or kept as a string if it doesn't parse; `tool_id = call_id`).
  - `custom_tool_call` → `tool_use` (`input` = `{"input": <string>}`).
  - `function_call_output` / `custom_tool_call_output` → `tool_result` (`text` = the output string, or the joined `text` of list items).
  - `reasoning` → `reasoning` only when `summary` has text; otherwise skipped.
- `ts` comes from the line's `timestamp`, or `""` if absent.

- [ ] **Step 1: Write failing tests** in `chat.rs` `#[cfg(test)] mod tests`, using literal JSON lines (`serde_json::json!`):
  - `claude_prompt_reply_tool`:
    - A user string prompt gives 1 entry, `kind == "you"`.
    - An assistant `[text, tool_use{id:"t1",name:"Bash",input:{command:"ls"}}]` gives kinds `["agent","tool_use"]`, with `tool_id == Some("t1")`.
    - A user `[tool_result{tool_use_id:"t1",content:"a\nb",is_error:true}]` gives `kind == "tool_result"`, `text == "a\nb"`, `is_error`.
  - `claude_skips_noise`:
    - `{"type":"api-request-blob",...}` gives `[]`.
    - A user `"<command-name>/clear</command-name>"` gives `kind == "system"`.
    - `isMeta:true` gives `system`.
    - Empty thinking gives `[]`.
  - `codex_message_and_calls`:
    - A `response_item/message` with role `user` and `input_text` "hi" gives `you` with "hi".
    - Role `developer` gives `system`.
    - A `function_call{name:"exec",arguments:"{\"cmd\":\"ls\"}",call_id:"c1"}` gives `tool_use` with `input["cmd"] == "ls"`.
    - A `custom_tool_call_output{call_id:"c1",output:[{type:"input_text",text:"ok"}]}` gives `tool_result` with "ok".
    - `event_msg` gives `[]`.
  - `caps_big_image_and_text`:
    - A tool_result whose image data is 3_000_001 chars gives `images[0].omitted == true` and `data == None`.
    - A text of 200_001 chars gives `truncated == true` and `text.chars().count() == 200_000`.
    - With `full = true`, neither cap applies.
- [ ] **Step 2: Run the tests to verify they fail:** `cd src-tauri && cargo test --lib chat::` gives a compile error / FAIL.
- [ ] **Step 3: Implement `normalize`** and the structs above, following the mapping rules.
- [ ] **Step 4: Run the tests:** `cargo test --lib chat::` passes, and the full `cargo test --lib` still passes (14 existing plus 4 new).
- [ ] **Step 5: Commit** with message `chat: normalise Claude and Codex transcript lines`.

### Task 2: Page and tail a transcript by byte offset

**Files:**
- Modify: `src-tauri/src/chat.rs`

**Interfaces:**
- Consumes: `normalize` (Task 1).
- Produces:
  ```rust
  #[derive(Serialize, Debug, Default)]
  pub struct Page {
      pub entries: Vec<ChatEntry>, // oldest first
      pub cursor: Option<u64>,     // pass as `before` for older entries; None = start of file reached
      pub end: u64,                // byte offset just after the last complete line read; pass as `from` to tail
      pub reset: bool,             // tail only: file is now shorter than `from`
      pub bad_lines: usize,
  }
  pub const PAGE_BYTES: u64 = 4 << 20;
  pub fn read_page(path: &Path, agent: &str, before: Option<u64>, limit: usize) -> Result<Page, String>
  pub fn read_tail(path: &Path, agent: &str, from: u64) -> Result<Page, String>
  pub fn read_entry(path: &Path, agent: &str, off: u64) -> Result<Vec<ChatEntry>, String> // full = true
  ```

**Behaviour:**
- **`read_page`:**
  - It starts at `before`, or at the end of the last complete line when `before` is `None`.
  - It steps backwards in windows of `PAGE_BYTES`. Within each window it splits on `\n`, and drops the leading fragment unless the window starts at 0.
  - It normalises lines newest to oldest until at least `limit` entries are collected, then returns them oldest first.
  - `cursor` is the offset of the oldest line it used, or `None` when it reached offset 0 with everything consumed.
  - A single line longer than `PAGE_BYTES` grows the window until the line is complete, so no line is ever lost.
- **`read_tail`:**
  - If `len < from`, it returns `reset: true` and no entries.
  - Otherwise it reads `[from, len)` and parses only complete lines (ending in `\n`). `end` is the offset after the last complete line.
- **Bad lines:** invalid JSON or invalid UTF-8 increments `bad_lines`, and the line is skipped.

- [ ] **Step 1: Write failing tests.** Each writes a temp file in `std::env::temp_dir()` with a unique name built from the test name and the process id, using Claude-format lines from Task 1's fixtures.
  - `page_newest_first_then_older`:
    - Setup: 10 prompt lines `p0..p9`.
    - `read_page(None, 3)` gives texts `["p7","p8","p9"]` with `cursor == Some(off_of_p7)`.
    - `read_page(cursor, 3)` gives `["p4","p5","p6"]`.
    - Paging on to the start gives `cursor == None`, and every line is seen exactly once.
  - `page_handles_line_bigger_than_window`:
    - Setup: one line of 5 MiB (a text block) between two small lines.
    - Paging back from the end returns all 3 entries, and the big one has `truncated == true`.
  - `tail_skips_partial_line`:
    - Setup: a file with `p0\n` plus a partial `{"type":"user",...` with no newline.
    - `read_tail(0)` gives 1 entry, with `end` equal to the length of the first line.
    - Append the rest of the line and `\n`; then `read_tail(end)` gives 1 entry.
  - `tail_reports_reset`: `read_tail(from = len + 100)` gives `reset == true` and no entries.
  - `bad_lines_counted`:
    - Setup: lines `p0`, `{not json`, `[0xff, 0xfe]` bytes followed by `\n`, then `p1`.
    - A page gives 2 entries and `bad_lines == 2`.
  - `read_entry_full`: a line with a 3_000_001-char image read with `read_entry(off)` gives `images[0].data.is_some()`.
- [ ] **Step 2: Run the tests:** `cargo test --lib chat::` fails.
- [ ] **Step 3: Implement** `read_page`, `read_tail` and `read_entry` with `std::fs::File`, `Seek` and `Read`, following the behaviour above.
- [ ] **Step 4: Run the tests:** `cargo test --lib` passes.
- [ ] **Step 5: Commit** with message `chat: page and tail transcripts by byte offset`.

### Task 3: Tauri commands, list snippets, and resuming a specific session

**Files:**
- Modify: `src-tauri/src/commands.rs` (append), `src-tauri/src/lib.rs` (register commands), `src-tauri/src/launcher.rs` (`LaunchReq` + continue branch)

**Interfaces:**
- Consumes: `read_page`, `read_tail`, `read_entry`, `Page` (Task 2).
- Produces (Tauri commands, camelCase args on the JS side):
  ```rust
  #[tauri::command(async)] pub fn chat_page(st: State<AppState>, id: String, before: Option<u64>, limit: Option<usize>) -> R<Value>
  // -> {"session": <sessions row + project name>, "page": Page}; limit default 120, max 400
  #[tauri::command(async)] pub fn chat_tail(st: State<AppState>, id: String, from: u64) -> R<Page>
  #[tauri::command(async)] pub fn chat_entry(st: State<AppState>, id: String, off: u64) -> R<Vec<ChatEntry>>
  #[tauri::command(async)] pub fn chat_list(st: State<AppState>, agent: Option<String>) -> R<Vec<Value>>
  // -> same rows as get_sessions, plus "last_reply": String (up to 160 chars) for the 60 most recently updated
  ```
  - **`LaunchReq`** gains `pub session_id: Option<String>` (`#[serde(default)]`). In `mode == "continue"`, when it's `Some`, look up that session with `SELECT id, cwd FROM sessions WHERE id=?1 AND project_id=?2 AND agent=?3` and use it instead of `latest_session`. If there's no match, the error is `"That chat doesn't belong to this project and agent."`. The existing id-character check still applies.

**Behaviour:**
- **Locating the transcript:** a helper `fn transcript(c: &Connection, id: &str) -> R<(PathBuf, String, Value)>` loads `path` and `agent` from `sessions`.
  - If the row is missing, the error is `"Unknown chat."`.
  - If `path` is null or the file doesn't exist, the error is `"This chat's transcript is no longer on disk."`.
- **`chat_list` snippets:** for the top 60 rows, `last_reply` comes from `read_page(path, agent, None, 40)` as the text of the last `agent` entry, or `""`. Errors become `""`.
- **Registration:** register all four commands in `lib.rs`'s `generate_handler!`.

- [ ] **Step 1: Write the failing test** `missing_file_message` in `commands.rs` tests, or in `chat.rs` if `transcript` lives there, which is the implementer's choice:
  - An in-memory DB via `db::migrate` with a `sessions` row whose `path` is `C:/nope/x.jsonl` gives `Err("This chat's transcript is no longer on disk.")`.
  - An unknown id gives `Err("Unknown chat.")`.
- [ ] **Step 2: Write the failing test** `continue_specific_session` in `launcher.rs` tests:
  - A session row `s2` belonging to project `a`/claude, plus a newer `s3`.
  - The helper that picks the session (extract `fn pick_session(conn, req) -> Result<(String, Option<String>), String>` from the continue branch) returns `s2` when `session_id == Some("s2")`, and `s3` when it's `None`.
  - `session_id == Some("other-project-session")` gives the "doesn't belong" error.
- [ ] **Step 3: Run the tests:** `cargo test --lib` fails.
- [ ] **Step 4: Implement** the commands, `transcript`, `pick_session` and the `LaunchReq.session_id` field.
- [ ] **Step 5: Run the tests:** `cargo test --lib` passes, and `cargo build` compiles.
- [ ] **Step 6: Commit** with message `chat: commands for pages, tail, full entry, list snippets; resume a chosen session`.

### Task 4: Pure display helpers (fold and markdown)

**Files:**
- Create: `src/chat/fold.ts`, `src/chat/fold.test.ts`, `src/chat/md.ts`, `src/chat/md.test.ts`

**Interfaces:**
- Produces:
  ```ts
  export type ChatEntry = { off: number; ts: string; kind: "you" | "agent" | "reasoning" | "system" | "tool_use" | "tool_result";
    text: string; tool_id?: string | null; name?: string | null; input?: any; is_error: boolean;
    images: { media_type: string; data?: string | null; omitted: boolean }[]; truncated: boolean };
  export type Step = { use: ChatEntry; result?: ChatEntry };
  export type Block =
    | { kind: "you" | "agent" | "reasoning" | "system"; entry: ChatEntry }
    | { kind: "tools"; key: string; steps: Step[]; counts: Record<string, number> };
  export function foldEntries(entries: ChatEntry[], showSystem: boolean): Block[];
  export function stepKind(name: string): "command" | "edit" | "read" | "search" | "other";
  export function toolSummary(counts: Record<string, number>): string;
  // md.ts
  export type MdBlock = { t: "code"; lang: string; body: string } | { t: "h"; level: number; text: string }
    | { t: "li"; ordered: boolean; text: string } | { t: "p"; text: string };
  export function mdBlocks(text: string): MdBlock[];
  export type Inline = { t: "text" | "code" | "bold"; s: string };
  export function inline(s: string): Inline[];
  ```

**Rules:**
- **`foldEntries`:**
  - Consecutive `tool_use`/`tool_result` entries form one `tools` block. A `tool_result` attaches to the `Step` whose `use.tool_id` matches, searching all earlier steps in the whole list, not just the current block. A result with no matching use becomes its own step with `use` = the result itself.
  - `system` entries are dropped when `showSystem` is false, and they don't break a tools run.
  - `key` is the `off` of the first step.
  - `counts` keys come from `stepKind`.
- **`stepKind`:**
  - `Bash`, `exec`, `shell` or `PowerShell` → command.
  - `Edit`, `Write`, `MultiEdit`, `NotebookEdit` or `apply_patch` → edit.
  - `Read` → read.
  - `Grep`, `Glob` or `WebSearch` → search.
  - Anything else → other.
- **`toolSummary`:**
  - Order is command, edit, read, search, other, joined with `" · "`.
  - Singular/plural pairs: `command(s)`, `edit(s)`, `read(s)`, `search(es)`, `other step(s)`.
- **`mdBlocks`:** handles fenced ``` blocks (with an optional language), `#`–`###` headings, `-`/`*`/`1.` list items, and blank-line-separated paragraphs. Single newlines inside a paragraph are kept in the text, so the view renders them with `white-space: pre-wrap`.
- **`inline`:** handles `` `code` `` and `**bold**`. Everything else is text.

- [ ] **Step 1: Write failing tests.**
  - `fold.test.ts`:
    - `toolSummary({command:4, edit:2, read:1})` returns `"4 commands · 2 edits · 1 read"`, and `toolSummary({search:1})` returns `"1 search"`.
    - For entries `[you, tool_use(t1,Bash), tool_result(t1), tool_use(t2,Edit), system, tool_result(t2), agent]`:
      - with `showSystem=false` → block kinds `["you","tools","agent"]`, with the tools block holding 2 steps, each with a `result`, and `counts {command:1, edit:1}`;
      - with `showSystem=true` → `["you","tools","system","tools","agent"]`.
    - An orphan `tool_result` with no use gives 1 step.
  - `md.test.ts`:
    - `mdBlocks("# Title\n\npara one\nline two\n\n- a\n- b\n\n```ts\nx<y\n```")` returns `[{t:"h",level:1,text:"Title"},{t:"p",text:"para one\nline two"},{t:"li",ordered:false,text:"a"},{t:"li",ordered:false,text:"b"},{t:"code",lang:"ts",body:"x<y"}]`.
    - `inline("use `npm i` **now**")` returns `[{t:"text",s:"use "},{t:"code",s:"npm i"},{t:"text",s:" "},{t:"bold",s:"now"}]`.
- [ ] **Step 2: Run the tests:** `npx vitest run src/chat` fails.
- [ ] **Step 3: Implement** both modules.
- [ ] **Step 4: Run the tests:** `npx vitest run src/chat` passes, and `npx tsc --noEmit` is clean.
- [ ] **Step 5: Commit** with message `chat: fold tool runs and minimal markdown (pure helpers)`.

### Task 5: CHAT view (list and conversation), nav, API

**Files:**
- Create: `src/views/Chat.tsx`
- Modify:
  - `src/api.ts`: add `chatList`, `chatPage`, `chatTail` and `chatEntry`, and add the optional `session_id` to `LaunchReq`.
  - `src/ui.tsx`: add `"chat"` to `View`.
  - `src/App.tsx`: add `["chat","CHAT"]` to `NAV` after `ACTIONS`, add `lazy(() => import("./views/Chat"))`, and route it.
  - `src/views/Projects.tsx`: add an "Open in CHAT" button in `SessionModal` that calls `nav("chat", { id })`.
  - `src/styles.css`: add `.chat-*` classes.

**Interfaces:**
- Consumes: the Task 3 commands, plus `foldEntries`, `toolSummary`, `mdBlocks` and `inline` from Task 4.
- Uses the existing `useAsync`, `useNav`, `ago`, `AgentChip`, `ErrBox`, `Empty`, `sessionState`, `api.openPath` and `api.launch`.

**Behaviour:**
- **List** (when `params.id` is unset):
  - Rows from `chatList(agent)`, with filter buttons ALL / CLAUDE / CODEX (the same `seg` markup as `Agents.tsx`), a project dropdown, and a text filter on title, project and last reply.
  - Rows whose `sessionState(updated).cls == "live"` are pinned on top with the existing `live` styling.
  - Each row shows: agent chip, project (or "unmapped"), title, the `last_reply` snippet in grey, `ago(updated)`, and prompts/edits counts.
  - Clicking a row calls `nav("chat", { id })`.
  - Refreshes on `projects-updated`.
- **Conversation** (`params.id` set):
  - **Header:** ← back to list, title, project, `cwd`, `branch`, agent.
  - **Header buttons:**
    - **Continue in terminal:** `api.launch({project_id, agent, mode:"continue", session_id: id})`. It's disabled when there's no `project_id`, with the tooltip "Map this chat to a project first".
    - **Open folder:** `api.openPath(cwd)`.
    - **Copy resume command:** `claude --resume <id>`, or `codex resume <id>`, as `cd "<cwd>"; ` + that, copied via `navigator.clipboard.writeText`.
  - **Toggles:** "Expand all tool steps" and "Show system entries".
  - **Load:** `chatPage(id)` on open, scrolled to the bottom. A "Load earlier" button at the top calls `chatPage(id, cursor)` and prepends the result, keeping the scroll position. It's hidden when `cursor` is null.
  - **Live:** every 3 s while mounted, call `chatTail(id, end)` and append the new entries. On `reset`, reload from scratch. Stick to the bottom only if the user was already within 80 px of it.
- **Blocks:**
  - `you`: a right-aligned bubble, with the full text in `pre-wrap`.
  - `agent`: the markdown rendered from `mdBlocks`/`inline` into React elements (code blocks as `<pre>`).
  - `reasoning`: folded "Reasoning" `<details>`.
  - `system`: dim small `pre-wrap`.
  - `tools`: a `<details>` whose summary is `toolSummary(counts)`. It's open when "Expand all" is on. Inside, each step shows:
    - the name;
    - `input.command`, or `input.file_path`, or the JSON input in `<pre>`;
    - for Edit, `old_string` and `new_string` as two `<pre>` blocks labelled "−" and "+";
    - the result text in `<pre>` (red border if `is_error`);
    - images as `<img src="data:<media_type>;base64,<data>">`, with a max width of 100%.
- **Truncated or omitted items:** an item marked `truncated` or `omitted` shows a "Load full" button that calls `chatEntry(id, off)` and swaps in that entry.
- **Errors:** errors from any call show in `ErrBox`. The missing-file message appears as is.

- [ ] **Step 1: Implement** the api additions, the nav entry and `Chat.tsx` as described. Keep the conversation rendering in `Chat.tsx`; split out a `ChatLog.tsx` only if the file passes about 300 lines.
- [ ] **Step 2: Type-check and build:** `npx tsc --noEmit` and `npx vite build` are clean, and `npx vitest run` passes.
- [ ] **Step 3: Commit** with message `chat: CHAT tab with full conversation log, live tail, resume this chat in terminal`.

### Task 6: Real check on the packaged app, then logs

**Files:**
- Vault: `02 Projects/Papertrail Command Center/` (all four files) and `MASTER_PASSDOWN.md` (the `### Papertrail Command Center` section plus a top line).

- [ ] **Step 1: Build:** `npm run tauri build` finishes with `Finished 1 bundle`.
- [ ] **Step 2: Run the release exe over CDP.** Set `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333` and drive it with playwright-core from `C:/Projectwork/OARA-TEST-ENV/node_modules`, as in previous sessions. Check:
  - **(a)** CHAT lists both Claude and Codex rows, with snippets.
  - **(b)** Opening session `66fb3832-88b9-4f7e-89f6-c43427f37a98` (202 MB) renders the first page in under 2 s, measured from the click to the first `.chat-block`, and "Load earlier" works twice.
  - **(c)** A Codex chat renders `you`/`agent`/tools blocks.
  - **(d)** Opening this very session while it's being written shows new entries within about 6 s.
  - **(e)** Take a screenshot to the scratchpad and look at it.
- [ ] **Step 3: Reinstall:** close the running copy, run the NSIS setup with `/S`, and relaunch.
- [ ] **Step 4: Update the vault logs:**
  - **TASKS:** the CHAT item now reads "phase 1 done; phase 2 (status + commits) next".
  - **PROJECT LOG:** a dated 🟦 CLAUDE entry with the commit hashes and the measured open time.
  - **DECISIONS:** a new 🟦 CLAUDE entry with the next free number, saying the CHAT phase 1 transcript reader uses byte-offset paging with no new dependencies.
  - **CONTINUITY LOG:** the "As of" line.
  - **Passdown:** Last Activity, Last Commit, a dated follow-up naming the touched files, and a top line.
  - Then run `python "C:\Projectwork\Book_Of_Secrets\10 SOPs\check_continuity_links.py"`, which should print `OK`.
- [ ] **Step 5: Commit** any test fixtures or tweaks with message `chat: phase 1 verified on packaged app`.
