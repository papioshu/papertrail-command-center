# PAPERTRAIL // COMMAND CENTER

A native Windows dashboard (Tauri 2 + Rust + React/TypeScript, WebView2, SQLite) for all registered projects:
startup briefing, project analytics, Claude Code and Codex session tracking, context-aware one-click CLI launches,
a Book of Secrets project graph, and a dependency-aware discovery radar. It runs locally, uses no API keys, and does
not need a dev server at runtime.

Continuity docs (canonical): Book of Secrets vault `02 Projects/Papertrail Command Center/`
(`CONTINUITY LOG.md`, `TASKS.md`, `PROJECT LOG.md`, `DECISIONS.md`). Visual source of truth: PapertrailOS
(`Book_Of_Secrets/02 Projects/PaperTrail OS/Papertrail OS/repo`, `DESIGN_SYSTEM.md`).

## Build from source

Prerequisites (Windows 11): Rust (MSVC toolchain), Visual Studio 2022 Build Tools with "Desktop development with C++"
and a Windows 11 SDK, Node 20+, the WebView2 runtime (ships with Windows 11).

```powershell
npm install
npm run tauri build      # -> src-tauri/target/release/papertrail-command-center.exe
                         # -> src-tauri/target/release/bundle/nsis/Papertrail Command Center_0.1.0_x64-setup.exe
cd src-tauri; cargo test --lib                                # unit tests
cargo test --lib real_index -- --ignored --nocapture          # full index of this machine into a temp DB
```

`npm run tauri dev` runs a development build with hot reload. The packaged app never needs it.

## Where things live

| Path | What |
|---|---|
| `src-tauri/src/registry.rs` | settings + project discovery (MASTER_PASSDOWN registry table, manual, root scan) |
| `src-tauri/src/continuity.rs` | read-only parsers: TASKS, DECISIONS, PROJECT LOG, CONTINUITY LOG |
| `src-tauri/src/gitscan.rs` | bounded `git status` / `git log --numstat`, workstream bucketing |
| `src-tauri/src/agents.rs` | Claude JSONL incremental parser (byte offsets), Codex `state_*.sqlite` reader, session→project mapping |
| `src-tauri/src/indexer.rs` | background worker: one job at a time, panic-contained, mtime/offset checkpoints |
| `src-tauri/src/launcher.rs` | handoff brief builder + Windows Terminal / console launcher, duplicate + conflict guards |
| `src-tauri/src/discovery.rs` | GitHub search + npm registry radar with ETags |
| `src-tauri/src/commands.rs` | Tauri command surface (the only API the UI has) |
| `src/views/*` | Overview, Projects, Agents, Launchpad, Graph, Discovery, Timeline, Settings |

App data: `%APPDATA%\os.papertrail.commandcenter\` (`papertrail.db`, `handoffs\`, `logs\app.log`, `backups\`).
