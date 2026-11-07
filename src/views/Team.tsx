import { useEffect, useState } from "react";
import { api, type Row } from "../api";
import { ago, Card, Empty, ErrBox, Modal, useAsync } from "../ui";

/** Shared team tasks on the self-hosted workspace hub (D030–D034). Buttons follow your role; the hub re-checks every edit. */

const COLS = [
  { key: "todo", label: "To do", color: "var(--violet-soft)" },
  { key: "doing", label: "In progress", color: "var(--violet)" },
  { key: "blocked", label: "Blocked", color: "var(--gold)" },
  { key: "done", label: "Done", color: "var(--lime)" },
] as const;
const RANK: Record<string, number> = { owner: 0, admin: 1, editor: 2, contributor: 3, viewer: 4 };
const can = (role: string, need: string) => (RANK[role] ?? 9) <= RANK[need];

function Join({ onDone }: { onDone: () => void }) {
  const [link, setLink] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const join = async () => {
    setBusy(true); setErr(null);
    try { await api.hubJoin(link.trim(), name.trim()); onDone(); } catch (e) { setErr(String(e)); } finally { setBusy(false); }
  };
  return (
    <Card title="Join your team's hub">
      <div className="col" style={{ gap: 10, maxWidth: 620 }}>
        <div className="small muted">Paste the invite link a teammate sent you (it looks like <span className="mono">http://100.x.y.z:8770#…</span>). It works once and expires.</div>
        <input className="input" placeholder="Invite link" value={link} onChange={(e) => setLink(e.target.value)} aria-label="Invite link" />
        <input className="input" placeholder="Your name, as your team will see it" value={name} onChange={(e) => setName(e.target.value)} aria-label="Your name" onKeyDown={(e) => e.key === "Enter" && join()} />
        <div className="row"><button className="btn primary" disabled={busy || !link.trim() || !name.trim()} onClick={join}>{busy ? "Joining…" : "Join"}</button></div>
        {err && <ErrBox error={err} />}
        <div className="dim small" style={{ lineHeight: 1.6 }}>
          Hosting the hub yourself: on the PC that will host it (Tailscale on), run <span className="mono">ptcc-hub new-workspace "Team name"</span>,
          then <span className="mono">ptcc-hub serve</span>, and paste the owner link it printed here. Your sessions, chats, files and notes never leave this PC; only tasks you put on the board are shared.
        </div>
      </div>
    </Card>
  );
}

function TaskCard({ t, ws, me, role, members, color, onErr, reload }: { t: Row; ws: string; me: string; role: string; members: Row[]; color: string; onErr: (e: string) => void; reload: () => void }) {
  const f = t.fields ?? {};
  const [editing, setEditing] = useState<Row | null>(null);
  const [confirm, setConfirm] = useState(false);
  const mine = t.created_by === me || !t.created_by;
  const editable = can(role, "editor") || (can(role, "contributor") && mine);
  const name = (id?: string) => members.find((m) => m.user_id === id)?.name ?? (id ? "someone" : "");
  const run = (action: "update" | "delete", fields: Row | null) => api.hubEdit(ws, action, "task", t.project_id, fields, t.id).then(reload, (e) => onErr(String(e)));
  if (editing)
    return (
      <div className="card col" style={{ padding: "10px 12px", gap: 6, borderLeft: `3px solid ${color}` }}>
        <input className="input" value={editing.title ?? ""} onChange={(e) => setEditing({ ...editing, title: e.target.value })} aria-label="Title" />
        <textarea className="input" rows={2} value={editing.notes ?? ""} onChange={(e) => setEditing({ ...editing, notes: e.target.value })} aria-label="Notes" />
        <div className="row" style={{ gap: 6 }}>
          {can(role, "editor") && (
            <select className="input" value={editing.assignee ?? ""} onChange={(e) => setEditing({ ...editing, assignee: e.target.value || null })} aria-label="Assign to">
              <option value="">Nobody</option>
              {members.map((m) => <option key={m.user_id} value={m.user_id}>{m.name}</option>)}
            </select>
          )}
          <input className="input" type="date" value={editing.due ?? ""} onChange={(e) => setEditing({ ...editing, due: e.target.value || null })} aria-label="Due date" />
        </div>
        <div className="row" style={{ gap: 6 }}>
          <button className="btn sm primary" disabled={!String(editing.title ?? "").trim()} onClick={() => {
            const changed: Row = {};
            for (const k of ["title", "notes", "assignee", "due"]) if ((editing[k] ?? null) !== (f[k] ?? null)) changed[k] = editing[k] ?? null;
            if (changed.notes === null) changed.notes = "";
            if (!can(role, "editor")) delete changed.assignee;
            (Object.keys(changed).length ? run("update", changed) : Promise.resolve()).then(() => setEditing(null));
          }}>Save</button>
          <button className="btn sm ghost" onClick={() => setEditing(null)}>Cancel</button>
        </div>
      </div>
    );
  return (
    <div className="card" style={{ padding: "10px 12px", borderLeft: `3px solid ${color}` }}>
      <div className="small" style={{ fontWeight: 600, textDecoration: f.status === "done" ? "line-through" : undefined }}>{f.title}</div>
      {f.notes && <div className="muted small" style={{ marginTop: 4, whiteSpace: "pre-wrap" }}>{f.notes}</div>}
      <div className="dim small" style={{ marginTop: 6, lineHeight: 1.5 }}>
        {f.assignee && <>for <b>{name(f.assignee)}</b> · </>}
        {f.due && <>due {f.due} · </>}
        {t.created_by ? <>by {name(t.created_by)}</> : <>new</>}
        {t.updated_at && <> · {ago(t.updated_at)}</>}
        {t.pending > 0 && <> · <span style={{ color: "var(--gold)" }}>waiting to sync</span></>}
      </div>
      {editable && (
        <div className="row wrap" style={{ marginTop: 8, gap: 6 }}>
          {COLS.filter((c) => c.key !== (f.status ?? "todo")).map((c) => (
            <button key={c.key} className="btn sm" onClick={() => run("update", { status: c.key })}>{c.label}</button>
          ))}
          <button className="btn sm ghost" onClick={() => setEditing({ ...f })}>Edit</button>
          {confirm ? <button className="btn sm danger" onClick={() => run("delete", null)}>Confirm delete</button> : <button className="btn sm ghost" onClick={() => setConfirm(true)}>Delete</button>}
        </div>
      )}
    </div>
  );
}

function Conflicts({ ws, list, reload, onErr }: { ws: string; list: Row[]; reload: () => void; onErr: (e: string) => void }) {
  if (!list.length) return null;
  const resolve = (id: number, keepMine: boolean) => api.hubResolve(ws, id, keepMine).then(reload, (e) => onErr(String(e)));
  return (
    <Card title={`Needs your choice (${list.length})`}>
      <div className="col" style={{ gap: 8 }}>
        {list.map((c) => (
          <div key={c.id} className="li" style={{ alignItems: "flex-start" }}>
            <div style={{ flex: 1 }}>
              {c.kind === "conflict" && <div className="small">Someone else changed <b>{Array.isArray(c.reason) ? c.reason.join(", ") : "this task"}</b> first. Theirs: <b>{c.theirs?.fields?.title}</b>. Yours: <span className="mono">{JSON.stringify(c.mine)}</span></div>}
              {c.kind === "deleted" && <div className="small">The task was deleted before your edit arrived. Your text: <span className="mono">{JSON.stringify(c.mine)}</span></div>}
              {c.kind === "rejected" && <div className="small">The hub refused this edit ({c.reason}). Your text is kept here: <span className="mono">{JSON.stringify(c.mine?.fields ?? c.mine)}</span></div>}
              <div className="dim small">{ago(c.at)}</div>
            </div>
            {c.kind === "conflict" && <button className="btn sm primary" onClick={() => resolve(c.id, true)}>Keep mine</button>}
            <button className="btn sm ghost" onClick={() => resolve(c.id, false)}>{c.kind === "conflict" ? "Keep theirs" : "Dismiss"}</button>
          </div>
        ))}
      </div>
    </Card>
  );
}

function Admin({ ws, board, reload, onErr }: { ws: string; board: Row; reload: () => void; onErr: (e: string) => void }) {
  const [role, setRole] = useState("editor");
  const [link, setLink] = useState<string | null>(null);
  const [pname, setPname] = useState("");
  const [local, setLocal] = useState("");
  const [preview, setPreview] = useState<Row | null>(null);
  const [doImport, setDoImport] = useState(true);
  const projects = useAsync(() => api.projects(), []);
  const roles = ["admin", "editor", "contributor", "viewer"].filter((r) => RANK[r] > RANK[board.role]);
  return (
    <Card title="Team admin">
      <div className="col" style={{ gap: 12 }}>
        <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
          <span className="label">Invite</span>
          <select className="input" value={role} onChange={(e) => setRole(e.target.value)} aria-label="Role">{roles.map((r) => <option key={r} value={r}>{r}</option>)}</select>
          <button className="btn sm" onClick={() => api.hubInvite(ws, role).then(setLink, (e) => onErr(String(e)))}>Make invite link</button>
          {link && <><input className="input mono" style={{ flex: 1, minWidth: 260 }} readOnly value={link} onFocus={(e) => e.target.select()} aria-label="Invite link" />
            <button className="btn sm" onClick={() => navigator.clipboard.writeText(link)}>Copy</button></>}
        </div>
        {link && <div className="dim small">Works once, for 72 hours. Send it privately: whoever opens it first joins as {role}.</div>}
        <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
          <span className="label">New shared project</span>
          <input className="input" placeholder="Name" value={pname} onChange={(e) => setPname(e.target.value)} aria-label="Project name" />
          <button className="btn sm" disabled={!pname.trim()} onClick={() => api.hubNewProject(ws, pname.trim()).then(() => { setPname(""); reload(); }, (e) => onErr(String(e)))}>Create</button>
        </div>
        <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
          <span className="label">Share a local project</span>
          <select className="input" value={local} onChange={(e) => setLocal(e.target.value)} aria-label="Local project">
            <option value="">Pick a project…</option>
            {(projects.data ?? []).map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
          </select>
          <button className="btn sm" disabled={!local} onClick={() => api.hubLinkPreview(local).then(setPreview, (e) => onErr(String(e)))}>Preview what's shared</button>
        </div>
      </div>
      {preview && (
        <Modal title={`Share "${preview.project_name}" with the team`} onClose={() => setPreview(null)}
          foot={<><button className="btn ghost" onClick={() => setPreview(null)}>Cancel</button>
            <button className="btn primary" onClick={() => api.hubLink(local, ws, doImport).then(() => { setPreview(null); setLocal(""); reload(); }, (e) => onErr(String(e)))}>Share</button></>}>
          <div className="col" style={{ gap: 8 }}>
            <div className="small">Exactly this goes to the hub:</div>
            <div className="small">• Project name: <b>{preview.project_name}</b></div>
            <label className="row small" style={{ gap: 6 }}><input type="checkbox" checked={doImport} onChange={(e) => setDoImport(e.target.checked)} /> Also add its {preview.tasks.length} action item{preview.tasks.length === 1 ? "" : "s"} as shared tasks (title, notes, status)</label>
            {doImport && preview.tasks.length > 0 && (
              <div className="col" style={{ gap: 2, maxHeight: 200, overflow: "auto" }}>
                {preview.tasks.map((t: Row, i: number) => <div key={i} className="mono small dim">[{t.status}] {t.title}{t.notes ? ` — ${t.notes}` : ""}</div>)}
              </div>
            )}
            <div className="dim small">Stays on this PC: {(preview.kept_private as string[]).join(", ")}.</div>
          </div>
        </Modal>
      )}
    </Card>
  );
}

function Board({ ws, me, onErr }: { ws: string; me: string; onErr: (e: string) => void }) {
  const board = useAsync(() => api.hubBoard(ws), [ws], ["hub-changed"]);
  const [project, setProject] = useState("");
  const [draft, setDraft] = useState({ title: "", assignee: "", due: "" });
  const b = board.data;
  useEffect(() => { if (b && !project && b.projects?.length) setProject(b.projects[0].id); }, [b, project]);
  if (board.error) return <ErrBox error={board.error} />;
  if (!b) return <div className="skeleton" />;
  const role: string = b.role ?? "viewer";
  const members: Row[] = b.members ?? [];
  const tasks: Row[] = (b.records ?? []).filter((r: Row) => r.kind === "task" && (!project || r.project_id === project));
  const add = () => {
    const fields: Row = { title: draft.title.trim(), status: "todo" };
    if (draft.assignee) fields.assignee = draft.assignee;
    if (draft.due) fields.due = draft.due;
    api.hubEdit(ws, "create", "task", project, fields).then(() => { setDraft({ title: "", assignee: "", due: "" }); board.reload(); }, (e) => onErr(String(e)));
  };
  return (
    <div className="col" style={{ gap: 14 }}>
      <Conflicts ws={ws} list={b.conflicts ?? []} reload={board.reload} onErr={onErr} />
      <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
        <select className="input" value={project} onChange={(e) => setProject(e.target.value)} aria-label="Shared project">
          {!(b.projects ?? []).length && <option value="">No shared projects yet</option>}
          {(b.projects ?? []).map((p: Row) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
        <span className="chip">{role}</span>
        <span className="dim small">{b.pending ? `${b.pending} change${b.pending === 1 ? "" : "s"} waiting to sync · ` : ""}{b.last_sync ? `synced ${ago(b.last_sync)}` : "not synced yet"}</span>
      </div>
      {can(role, "contributor") && project && (
        <div className="row wrap" style={{ gap: 8 }}>
          <input className="input" style={{ flex: 2, minWidth: 240 }} placeholder="New shared task…" value={draft.title} onChange={(e) => setDraft({ ...draft, title: e.target.value })} onKeyDown={(e) => e.key === "Enter" && draft.title.trim() && add()} aria-label="Task title" />
          {can(role, "editor") && (
            <select className="input" value={draft.assignee} onChange={(e) => setDraft({ ...draft, assignee: e.target.value })} aria-label="Assign to">
              <option value="">Nobody</option>
              {members.map((m) => <option key={m.user_id} value={m.user_id}>{m.name}</option>)}
            </select>
          )}
          <input className="input" type="date" value={draft.due} onChange={(e) => setDraft({ ...draft, due: e.target.value })} aria-label="Due date" />
          <button className="btn primary" disabled={!draft.title.trim()} onClick={add}>Add</button>
        </div>
      )}
      {!(b.projects ?? []).length && <Empty>{can(role, "admin") ? "Create a shared project or share a local one below to start the board." : "No shared projects you can see yet."}</Empty>}
      {project && (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(4, minmax(0, 1fr))", gap: 14 }}>
          {COLS.map((c) => {
            const list = tasks.filter((t) => (t.fields?.status ?? "todo") === c.key);
            return (
              <div key={c.key} className="col" style={{ gap: 8 }}>
                <div className="row" style={{ borderBottom: `2px solid ${c.color}`, paddingBottom: 6 }}>
                  <span className="label" style={{ color: c.color }}>{c.label}</span>
                  <span className="mono dim" style={{ marginLeft: "auto" }}>{list.length}</span>
                </div>
                <div className="col" style={{ gap: 8, maxHeight: 560, overflow: "auto", paddingRight: 2 }}>
                  {list.map((t) => <TaskCard key={t.id} t={t} ws={ws} me={me} role={role} members={members} color={c.color} onErr={onErr} reload={board.reload} />)}
                  {!list.length && <div className="dim small mono" style={{ padding: 8 }}>none</div>}
                </div>
              </div>
            );
          })}
        </div>
      )}
      {can(role, "admin") && <Admin ws={ws} board={b} reload={board.reload} onErr={onErr} />}
    </div>
  );
}

export default function Team() {
  const status = useAsync(() => api.hubStatus(), [], ["hub-changed"]);
  const [ws, setWs] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [leaving, setLeaving] = useState(false);
  const s = status.data;
  const list: Row[] = s?.workspaces ?? [];
  useEffect(() => { if (list.length && !list.some((w) => w.id === ws)) setWs(list[0].id); }, [list, ws]);
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="page-head">
        <div><h1>Team</h1><div className="sub">Shared tasks on your own hub · only what you put on the board leaves this PC</div></div>
        {s?.joined && (
          <div className="row" style={{ gap: 8, marginLeft: "auto", alignItems: "center" }}>
            <span className="small"><span className={`dot ${s.online === false ? "busy" : ""}`} /> {s.online === false ? "Hub offline · showing last sync" : s.online ? "Hub online" : "Connecting…"}</span>
            <button className="btn sm" onClick={() => api.hubSyncNow()}>Sync now</button>
            {leaving
              ? <button className="btn sm danger" onClick={() => api.hubLeave().then(() => { setLeaving(false); status.reload(); }, (e) => setErr(String(e)))}>Confirm: leave hub on this PC</button>
              : <button className="btn sm ghost" onClick={() => setLeaving(true)}>Leave</button>}
          </div>
        )}
      </div>
      {err && <div onClick={() => setErr(null)}><ErrBox error={err} /></div>}
      {status.error && <ErrBox error={status.error} />}
      {s && !s.joined && <Join onDone={status.reload} />}
      {s?.revoked && <ErrBox error="This PC was signed out of the hub. Your unsent edits were saved in the app's hub\unsent folder. Leave, then join again with a new invite." />}
      {s?.joined && !s.revoked && !list.length && <Empty>You're not in any workspace on this hub right now.</Empty>}
      {list.length > 1 && (
        <div className="row wrap" style={{ gap: 6 }}>
          {list.map((w) => <button key={w.id} className={`btn sm ${w.id === ws ? "primary" : ""}`} onClick={() => setWs(w.id)}>{w.name}</button>)}
        </div>
      )}
      {ws && s?.joined && <Board key={ws} ws={ws} me={s.user_id ?? ""} onErr={setErr} />}
    </div>
  );
}
