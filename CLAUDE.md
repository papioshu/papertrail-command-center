# CLAUDE.md — Papertrail Command Center

Startup: read `AGENTS.md` (ownership), then the vault continuity set in
`C:\Projectwork\Book_Of_Secrets\02 Projects\Papertrail Command Center\` and the `### Papertrail Command Center` passdown entry.

- Release gates: performance and reliability. If a feature threatens either, simplify it.
- Indexing is read-only. The app writes to external files only through explicit, previewed actions (Discovery › Add to BoS).
- Never put project text on a command line: the launcher passes only the path of a generated brief.
- After Rust changes: `cd src-tauri; cargo test --lib`. Real-data check: `cargo test --lib real_index -- --ignored --nocapture`.
- After UI changes: `npx tsc --noEmit && npx vite build`. Release: `npm run tauri build`.
- Machine RAM is often near full; builds and tests can be slow. Rerun before assuming a code fault.
