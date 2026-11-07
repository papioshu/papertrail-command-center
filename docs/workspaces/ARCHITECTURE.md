# Multi-user workspaces — data ownership, privacy and access control

Status: **Milestone 0, architecture.** Decided in D030. Only the access-control core
(`crates/access`) is built so far. The hub, sync and UI are later milestones (§10).
Owner brief: self-hosted and optional; no Supabase or other hosted backend-as-a-service; the desktop exe stays light and
local-first.

## 1. What exists today (inspected 2026-11-07)

| Piece | What it is | What it means for workspaces |
|---|---|---|
| `projects.workspace` | A display grouping derived from vault folders, the passdown registry and `workspace_groups` (`registry.rs`). | A label only, not a security boundary. A hub workspace is a separate object that a local project is *linked* to (§6). |
| `TEAM.json` profiles (D019) | Name + PIN in the vault; records who did what. | Not authentication. Stays as the local sign-in label. Hub accounts are separate (§4). |
| `ACTION ITEMS.md` (team.rs) | Per-project items in the vault, written atomically by the app. | First candidate for publishing into a workspace (§6.3). |
| Tasks, events, sessions, links, search, graph (`db.rs`) | A local cache rebuilt from local files. | Machine-private by default. Never synced as-is (§3). |
| Phone access (D027–D029) | Read-only, owner-only, device bearer keys (hashed) over Tailscale; the standby can only start the app. | Unchanged. It stays owner-only and separate from the hub. Its key-hash and revoke pattern is reused for hub devices. |
| Launcher | Starts Claude Code / Codex locally with a brief file (no project text on the command line). | Each collaborator uses their own CLI. A brief may include authorized workspace data. The hub never launches anything (§8). |

## 2. Components

```
 ┌───────────── each member's PC ─────────────┐        ┌──── hub (optional, self-hosted) ────┐
 │ Command Center (Tauri, unchanged core)     │        │ ptcc-hub (Rust, single binary)      │
 │  local SQLite cache  ← local files (P0/P1) │  HTTPS │  SQLite (WAL), media dir            │
 │  hub module (off when no hub configured):  │ ─────► │  authn: device sessions             │
 │   outbound queue, workspace cache, cursor  │  over  │  authz: crates/access on EVERY      │
 │  crates/access (UX hints only)             │Tailscale│   read/write/search/graph/download │
 └────────────────────────────────────────────┘        └─────────────────────────────────────┘
```

* **Desktop:** the existing app. The hub module creates no threads, opens no connections and adds no tables until a hub URL
  is configured. With the hub offline, everything local keeps working and workspace views show the last cached state.
* **Hub:** a separate binary (`hub/`, a later milestone). It is never bundled into the desktop installer. By default it
  binds a Tailscale address only (§9).
* **`crates/access`:** the pure policy library: roles, permissions, decisions, graph filtering and publication rules.
  The hub enforces with it; the desktop uses it only to hide buttons. No I/O, no dependencies.

## 3. Data ownership and privacy classes

Every piece of data has exactly one class. Anything not listed is **P0**: the default is private.

| Class | Owner / authority | Examples | Leaves the machine? |
|---|---|---|---|
| **P0 Machine-private** | This PC | CLI session histories and transcripts (`sessions`, `session_*`, CHAT), credentials and tokens, settings, launch history, file paths, repo working state, discoveries, index errors, phone devices, private Book of Secrets notes | **Never.** No code path serializes P0 to the hub. |
| **P1 Personal project** | The user | Every local project until it is explicitly linked to a workspace | No. Linking is an explicit owner action (§6.1). |
| **P2 Workspace-shared** | **Hub is authoritative** | Workspace, members, roles, shared tasks, milestones, assignments, progress, published summaries, approved docs, media, activity metadata, audit log | Yes, only to members authorized for that workspace and project. |

Rules:
1. **Publish by allowlist, never by blocklist.** Outbound data is built from a fixed set of record kinds (`Publishable` in
   `crates/access`). There is no generic "sync this row" path. Adding a kind takes code review and a DECISIONS entry.
2. **Field-level publication.** A linked project shares only the fields its link manifest lists. Paths, branch names,
   commit SHAs, author emails and file lists are stripped unless the manifest names them.
3. **Activity metadata is coarse.** "N commits today by member X on project Y". No commit messages, diffs or transcript
   text.
4. **Documents are approved one by one.** A vault document is shared only as a reviewed snapshot (the same
   preview-before-write pattern as Discovery › Add to BoS). It is never synced live, and the vault folder is never
   mounted.
5. **Hub data cached on a member's PC is still P2.** It lives in a separate cache file per workspace
   (`hub/<workspace_id>.db`), so revocation can delete it whole (§7.5).

## 4. Identity, devices and credentials

* **Account:** a hub user (`user_id`, display name). There are no passwords: a user is created by redeeming an
  invitation.
* **Invitation:** a random 256-bit token shown once (link or QR). The hub stores only its SHA-256, the workspace, the role,
  an optional project scope, an expiry (default 72 h, max 7 days), `max_uses = 1`, and who issued it. Redeeming is atomic
  (single-use check and insert in one transaction). Expired, used or revoked invitations all give the same error.
* **Device session:** redeeming (or adding a device from an existing one) creates a device with a random 256-bit secret.
  The hub stores its hash, as phone pairing already does. The desktop stores the secret in the OS credential store
  (Windows Credential Manager / DPAPI), never in SQLite or settings. Sessions can be listed and revoked per device; revoking
  a user revokes all their devices.
* **Bearer per request,** constant-time hash comparison. No cookies; the desktop is the only client at first.
* **TEAM.json profiles stay** as the local label for who did what on a shared PC. They grant nothing on the hub.

## 5. Authorization model (`crates/access`)

**Resources:** `Workspace` ⊃ `Project` ⊃ {`Task`, `Milestone`, `Doc`, `Media`}, plus `Member`, `Invitation` and
`Audit`.

**Roles** (per workspace membership; one user can hold different roles in different workspaces):

| Permission | owner | admin | editor | contributor | viewer |
|---|:-:|:-:|:-:|:-:|:-:|
| workspace.read, project.read, task.read, doc.read, media.read | ✓ | ✓ | ✓ | ✓ | ✓ |
| task.create, task.edit_own, media.upload | ✓ | ✓ | ✓ | ✓ | |
| task.edit_any, task.assign, milestone.edit, doc.publish | ✓ | ✓ | ✓ | | |
| project.link, members.invite, audit.read | ✓ | ✓ | | | |
| members.manage (roles, revoke), workspace.manage | ✓ | ✓¹ | | | |
| workspace.delete, ownership transfer | ✓ | | | | |

¹ An admin can't change or revoke an owner or another admin. A workspace always keeps at least one owner.

**Custom permissions:** a membership can carry explicit `grant` and `deny` sets on top of its role. **Deny beats
grant beats role.** Owner-only actions can't be granted.

**Project scope:** a membership may be limited to a list of projects. Outside that list, the member's project-level
permissions don't apply, and those projects don't exist for them.

**Decision procedure** (`access::decide`), deny by default:
1. No active membership for the workspace → `NotFound`.
2. The resource's project is outside the membership's scope → `NotFound` (never `Forbidden`: no existence leak).
3. The permission is in `deny` → `Forbidden` (the member can see the resource but can't do this).
4. The role or `grant` includes the permission, and the action-specific rules pass (edit_own: the actor created the
   task; admin vs owner protection) → `Allow`.
5. Otherwise → `Forbidden` if the member can read the resource, else `NotFound`.

**Enforced server-side, every time:** every hub handler calls `decide` before touching data. List, search and change
queries are scoped in SQL by the caller's authorized project set (a join, not a post-filter), so unauthorized rows are
never read into memory. Media downloads check `media.read` on the owning project. Change-feed subscriptions recheck on
every poll, so a revocation takes effect on the next request.

**Graph:** `access::filter_graph` keeps only nodes the caller can read, and only edges whose **both** ends are kept.
Node labels come from the authorized record. Hidden nodes don't appear as counts, "+N more" or dangling edge ids.
Search uses the same authorized set, so a hidden project's name can't be inferred from search results either.

## 6. Linking local work into a workspace

1. **Link:** someone with `project.link` links a local project to a hub workspace project. The link is local
   (`hub_links`: local project id → hub workspace/project id, plus the field manifest) and is created only by an explicit
   action with a preview of exactly what will be sent.
2. **Unlink** stops publishing. Data already on the hub stays (it is workspace-owned) until an admin deletes it there.
3. **Action items → shared tasks:** first publish imports the project's `ACTION ITEMS.md` items as hub tasks. After
   that, the hub is authoritative for shared tasks. The vault file becomes a local mirror the app writes, so offline
   reading still works. Edits made directly in the file are not pushed back (a one-way mirror is simpler and can't
   corrupt the hub).

## 7. Sync

### 7.1 Records and versions
Every hub record has `id` (UUID v4, made by the client so offline creates are possible), `version` (incremented by the
hub on each write), `updated_by`, `updated_at` and `deleted` (tombstone). Each workspace has a monotonic change sequence
`seq`.

### 7.2 Outbound queue (desktop)
Table `outbox(op_id UUID PRIMARY KEY, workspace_id, kind, record_id, base_version, payload JSON, created_at, attempts,
next_attempt, last_error)`. Each local edit is written to the cache and the outbox in one transaction, so it survives
crashes and restarts. A worker sends ops in order per record. Retries back off exponentially (5 s → 15 min, with jitter),
and only while the hub is reachable. `op_id` makes retries idempotent: the hub keeps applied op ids (`applied_ops`, 30-day
window) and answers a duplicate with the original result.

### 7.3 Inbound
`GET /v1/workspaces/{id}/changes?since={seq}` returns authorized changes after `seq` (paged, capped). The cursor is
stored per workspace. No WebSocket or SSE.

### 7.4 Conflicts
The client sends `base_version`. If the hub has moved on:
* **Disjoint fields** (e.g. one member changed status, another changed the assignee) → the hub merges per field and
  accepts.
* **Same field** → the hub answers `409` with the current record. The client keeps the local change as a **conflict copy**
  on that record, shown to the author with "keep mine / keep theirs". Nothing is silently dropped.
* **Delete vs edit** → the delete wins on the hub; the editor's text is kept as a conflict copy on their machine.

### 7.5 Revocation
If the change feed or any call returns `revoked` for a workspace (or the device is revoked):
1. Pending outbox ops for that workspace are rejected by the hub (authorization is checked at apply time, not queue
   time).
2. The client moves the user's **own unsent ops** to a local P1 file
   (`unsent/<workspace>-<date>.json`, the user's own authored text only), so their work isn't lost.
3. The client deletes the workspace cache file (`hub/<workspace_id>.db`) and its media cache. No removed workspace data
   stays readable.
A project removed from a member's scope is handled the same way, per project, using tombstones in the change feed.

## 8. Claude Code / Codex for collaborators
* Each collaborator launches **their own** authenticated CLI on **their own** PC through the existing launcher. The
  brief may include authorized workspace data (tasks, milestones, approved docs) read from their local workspace cache.
* The hub has **no** endpoint that runs a command, opens a shell, starts a CLI or reaches another member's PC. Membership
  never grants execution. This is a test-enforced invariant (the permission list contains no execute permission;
  `access` tests assert it).
* The owner's phone access and phone standby stay owner-only and are never reachable through hub credentials.

## 9. Network, deployment and resources
* **Hub binding:** Tailscale address (and loopback) only by default. Public exposure needs an explicit config flag, and is
  documented as not recommended. Transport: Tailscale's WireGuard. Optional `tailscale serve` HTTPS. Application-level auth
  (§4) is required on every request regardless of network.
* **Polling:** the change feed is polled every 60 s while the window is visible, every 10 min while hidden in the tray,
  and immediately on window focus and after a local write. With no hub configured: no requests at all. Requests are
  HTTP/1.1 keep-alive on one connection; nothing is held open between polls.
* **Desktop cost:** one sync thread, started only when a hub is configured. No new dependency in the desktop unless a
  DECISIONS entry approves it (the credential store will need one, or DPAPI through the Windows crate Tauri already
  links).

## 10. Hub storage: SQLite evaluation

| Question | Assessment |
|---|---|
| Write concurrency | SQLite allows one writer at a time. Hub writes are small (task edits): thousands per second sequentially against a team of tens making a few per minute. WAL lets reads run during writes. One writer connection behind a mutex plus a small read pool; `busy_timeout` set. **Adequate.** |
| Durability / backups | WAL + `synchronous=FULL` on the hub. Nightly online backup (`VACUUM INTO` a dated file) plus a backup before every migration. Keep 14 daily copies plus monthly ones. Media is content-addressed files (sha256 names) backed up with the same rotation. A restore drill is part of the hub milestone's acceptance. |
| Migrations | Same pattern as the desktop `db.rs` (ordered migrations, `user_version`). Each migration runs in a transaction after an automatic backup. Additive by default: columns are added, never repurposed. |
| Limits | One node, one disk. Fine up to roughly 50–100 active users and low hundreds of GB of media. Move to Postgres if multi-node, sustained high write rates or >100 concurrent writers are ever needed. The authz and sync layers don't depend on SQLite specifics. |
| Search | FTS5 per workspace, queried only through the authorized project set. |

**Selected:** Rust + SQLite for the first hub. Postgres is the documented upgrade path, not built.

## 11. Audit
An append-only `audit(seq, ts, workspace_id, actor_user, actor_device, action, target, detail JSON)` on the hub covers
invitations created/redeemed/revoked, membership and role changes, grants/denies, project links, publishes, deletions,
device sessions, logins from new devices and authorization denials (rate-limited). It is readable with `audit.read`.
Nothing in the API can update or delete audit rows.

## 12. Compatibility and migration
* No hub configured = today's app, byte for byte in behaviour. New desktop tables are created only on first hub setup
  (a separate migration step that is a no-op otherwise).
* Existing `workspace` labels, TEAM.json, ACTION ITEMS.md, phone access and standby are unchanged.
* The desktop never requires the hub. Hub-down shows "Workspace offline · showing last sync <time>".

## 13. Milestones
| # | Scope | Gate |
|---|---|---|
| **M0** | This document + `crates/access` (roles, permissions, decide, graph filter, publication allowlist) with tests | `cargo test` in `crates/access` |
| M1 | `hub/` binary: schema, migrations, backups, invitations, devices, memberships, audit, authz on every route; no sync yet | Integration tests: every route × role × scope matrix; revoked device 401; hidden project 404; restore drill |
| M2 | Shared tasks/milestones/media: change feed, versions, conflicts, idempotent ops | Two-client tests: offline edits, conflicting edits, duplicate retries, revocation mid-queue |
| M3 | Desktop hub module: configure/join (invite link), credential store, outbox, per-workspace cache, link-project preview | Exe size and startup unchanged with no hub configured (measured); hub-offline run |
| M4 | UI in PapertrailOS language: workspace switcher, members/roles/invites, access controls, shared project dashboard, permission-aware graph | Visual pass against DESIGN_SYSTEM.md; graph/search leak tests through the UI |
