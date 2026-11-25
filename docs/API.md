# Command Center workspace API (for your own dashboard)

The Command Center hub is a small HTTP + JSON server on Paper's PC. With an **API key** your own dashboard app can
read a workspace's projects, tasks, milestones, members and media, and (if your key allows it) create and edit tasks
and milestones.

## Before you start

1. **Tailscale.** The hub only answers on Paper's Tailscale network. Install Tailscale on the machine your dashboard
   runs on and accept Paper's invite. Nothing is reachable from the open internet.
2. **An account.** Join a workspace with the invite link Paper sends you.
3. **A key.** Paper makes it in Command Center: TEAM › PEOPLE › your name › **API keys**. You get the key once,
   plus the hub address (for example `https://paper.tail644a44.ts.net`). Keep the key secret, like a password.

A key is one of:

| Access | Can do |
|---|---|
| `read` | Read the workspaces it was made for. |
| `tasks` | The same, plus create, edit and delete **tasks and milestones**. |

A key never does more than your own role allows, only reaches the workspaces ticked when it was made, and can be
revoked at any time. Uploads, boards, members, invites and admin are not available to keys.

## Calling it

Send the key on every request:

```
Authorization: Bearer k_0123….abcd…
```

Replies are JSON. Errors look like `{ "error": "forbidden" }` with an HTTP status:
`401` bad or revoked key · `403` not allowed for this key or your role · `404` not found (or hidden from you) ·
`400` bad input · `409` conflict (see editing).

Browser dashboards can call the API directly (CORS is open for these endpoints). Desktop or server apps just send
the header.

```js
const HUB = "https://paper.tail644a44.ts.net";
const KEY = "k_…";   // keep out of source control
const hub = (path, init = {}) =>
  fetch(HUB + path, { ...init, headers: { Authorization: `Bearer ${KEY}`, "Content-Type": "application/json", ...init.headers } })
    .then(async (r) => { const b = await r.json(); if (!r.ok) throw new Error(`${r.status} ${b.error}`); return b; });

const me = await hub("/v1/me");
console.log(me.workspaces);   // the workspaces this key can see
```

## Reading

| Request | Returns |
|---|---|
| `GET /v1/me` | `{ user_id, name, workspaces: [{ id, name, role, title }], api_key: "read" \| "tasks" }` |
| `GET /v1/workspaces/{ws}` | `{ id, name, role, scoped }` |
| `GET /v1/workspaces/{ws}/projects` | `{ projects: [{ id, name, … }] }` |
| `GET /v1/workspaces/{ws}/projects/{p}` | one project |
| `GET /v1/workspaces/{ws}/members` | `{ members: [{ user_id, name, role, title }] }` |
| `GET /v1/workspaces/{ws}/changes?since=0&limit=200` | the record feed (below) |
| `GET /v1/workspaces/{ws}/media/{id}` | a media file's bytes (download) |

### The change feed: everything in one call

`changes` returns every record you may see, oldest change first:

```json
{ "changes": [ { "id": "…", "kind": "task", "project_id": "p_…", "version": 3, "seq": 812,
                 "fields": { "title": "Order merch", "status": "doing", "assignee": "u_…", "due": "2026-12-01", "notes": "…" },
                 "created_by": "u_…", "updated_by": "u_…", "updated_at": "2026-11-24T18:02:11Z", "deleted": false } ],
  "next": 812, "more": false, "projects": [ … ] }
```

- First load: `since=0`, then keep calling with `since=<next>` while `more` is true.
- Staying current: poll every 10–60 s with `since=<last next>`; you only get what changed. A record that was
  deleted comes back with `"deleted": true`; drop it from your view.
- Keep the newest copy of each `id` (the highest `version`).
- `kind`: `task`, `milestone`, and (if your role reads them) `media`, `board`, `board_el`, `board_note`, `snapshot`,
  `file`, `proposal`. A dashboard usually wants `task` and `milestone`.

Task fields: `title` (1–200 chars), `status` (`todo`, `doing`, `blocked`, `done`), `assignee` (a member's
`user_id` or null), `due` (`YYYY-MM-DD` or null), `notes` (≤ 4000), `milestone` (a milestone id in the same project or
null). Milestone fields: `title`, `status` (`open`, `done`), `due`, `progress` (0–100).

## Editing tasks and milestones (`tasks` keys)

Every change is one **op**, sent to `POST /v1/workspaces/{ws}/ops`. Each op carries two UUIDs you make
(`crypto.randomUUID()`): `op_id`, unique per attempt (retrying the same `op_id` is safe, it applies once), and
`record_id`, the record's id (new for a create).

Create:
```json
{ "op_id": "…", "record_id": "…", "kind": "task", "action": "create", "project_id": "p_…",
  "fields": { "title": "Shoot the promo", "status": "todo", "due": "2026-12-05" } }
```

Update (send only the fields you change, plus the `version` you last saw as `base_version`):
```json
{ "op_id": "…", "record_id": "<task id>", "kind": "task", "action": "update", "base_version": 3,
  "fields": { "status": "done" } }
```

Delete:
```json
{ "op_id": "…", "record_id": "<task id>", "kind": "task", "action": "delete" }
```

- Success replies `{ "record": { … } }` with the new `version`.
- Edits to different fields by two people both apply. If someone changed the **same** field since your
  `base_version`, you get `409` with `{ "error": "conflict", "fields": [ … ], "record": { …current… } }`: show it,
  then retry with the current `version` as `base_version`.
- Your role still applies, exactly as in the app (for example, contributors may claim and unclaim tasks themselves,
  but only editors and up assign them to others).
- Unknown fields are refused (`400`); nothing beyond the fields listed above is stored.

## Good manners

- Poll the feed, don't hammer it: every 10 s at most.
- Store the key in your app's settings or an environment variable, never in a repo or a web page others can open.
- Lost or leaked key: tell Paper; revoking it stops it at once. A new key is a click.
