# AGENTS.md — Codex brief for Papertrail Command Center

You are 🟧 CODEX on a repo that Claude Code (🟦 CLAUDE) built and maintains. Read this file, then the vault continuity
set: `C:\Projectwork\Book_Of_Secrets\02 Projects\Papertrail Command Center\` → `CONTINUITY LOG.md`, `TASKS.md`,
`DECISIONS.md`. The central passdown is `C:\Projectwork\Book_Of_Secrets\MASTER_PASSDOWN.md` → `### Papertrail Command Center`.
Never create another MASTER_PASSDOWN. Tag your own log entries 🟧 CODEX; never retag or rewrite Claude's.

## File ownership (no simultaneous edits to shared files)

| Owner | Paths |
|---|---|
| 🟦 CLAUDE | `src-tauri/**` (all Rust), `src/api.ts`, `src/App.tsx`, `src/ui.tsx`, `package.json`, `vite.config.ts`, `tsconfig.json` |
| 🟧 CODEX (claimable) | `src/views/Graph.tsx`, `src/views/Discovery.tsx`, `src/styles.css` visual refinements, new files under `src/assets/` |

Claim a path in `TASKS.md` NOW (tagged 🟧 CODEX) before editing it, and work on a `codex/*` branch. Anything outside
your column: write the request into `TASKS.md` instead of editing.

## Rules

- Visual authority is PapertrailOS (`02 Projects/Papertrail Command Center/PaperTrail OS/Papertrail OS/repo/DESIGN_SYSTEM.md`, `src/app/globals.css`,
  `assets/papertrail-os-landing.png`). Reuse its tokens; no new gradients, glows, or generic dashboard styling.
- No fake data. Every number on screen must come from a backend command (`src/api.ts`).
- No new runtime dependencies without a DECISIONS entry.
- Verify with `npx tsc --noEmit` and `npx vite build`; the Rust side with `cd src-tauri; cargo test --lib`.
