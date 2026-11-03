import { useState } from "react";
import { api, type ActionOp, type Row } from "../api";
import { ago, Empty, ErrBox, useAsync } from "../ui";

const COLS = [
  { key: "open", label: "To do", color: "var(--violet-soft)" },
  { key: "doing", label: "In progress", color: "var(--violet)" },
  { key: "done", label: "Done", color: "var(--lime)" },
] as const;

/** Shared action items (ACTION ITEMS.md in each project's vault folder). One project, or all of them. */
export function ActionBoard({ projectId }: { projectId?: string }) {
  const items = useAsync(() => api.actions(projectId), [projectId], ["projects-updated"]);
  const team = useAsync(() => api.team(), []);
  const projects = useAsync(() => (projectId ? Promise.resolve([]) : api.projects()), [projectId]);
  const [err, setErr] = useState<string | null>(null);
  const [draft, setDraft] = useState({ title: "", notes: "", assignee: "", project: "" });
  const [editing, setEditing] = useState<Row | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const people: Row[] = team.data?.profiles ?? [];
  const homes = (projects.data ?? []).filter((p) => p.vault_dir);

  async function run(pid: string, op: ActionOp) {
    setErr(null);
    try { await api.actionOp(pid, op); items.reload(); return true; } catch (e) { setErr(String(e)); return false; }
  }
  async function add() {
    const pid = projectId ?? draft.project;
    if (!pid) return setErr("Pick a project for this item.");
    if (await run(pid, { kind: "add", title: draft.title, notes: draft.notes, assignee: draft.assignee })) setDraft({ ...draft, title: "", notes: "" });
  }

  const all = items.data ?? [];
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="row wrap" style={{ gap: 8, alignItems: "flex-end" }}>
        {!projectId && (
          <select className="input" value={draft.project} onChange={(e) => setDraft({ ...draft, project: e.target.value })} aria-label="Project">
            <option value="">Project…</option>
            {homes.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
          </select>
        )}
        <input className="input" style={{ flex: 2, minWidth: 220 }} placeholder="New action item…" value={draft.title}
          onChange={(e) => setDraft({ ...draft, title: e.target.value })} onKeyDown={(e) => e.key === "Enter" && add()} aria-label="Action item title" />
        <input className="input" style={{ flex: 2, minWidth: 160 }} placeholder="Notes (optional)" value={draft.notes} onChange={(e) => setDraft({ ...draft, notes: e.target.value })} aria-label="Notes" />
        <select className="input" value={draft.assignee} onChange={(e) => setDraft({ ...draft, assignee: e.target.value })} aria-label="Assign to">
          <option value="">Anyone</option>
          {people.map((p) => <option key={p.id} value={p.name}>{p.name}</option>)}
        </select>
        <button className="btn primary" disabled={!draft.title.trim()} onClick={add}>Add</button>
      </div>
      {err && <ErrBox error={err} />}
      {items.error && <ErrBox error={items.error} />}
      {items.data && !all.length && <Empty>No action items yet. Add the first one above.</Empty>}
      <div style={{ display: "grid", gridTemplateColumns: "repeat(3, minmax(0, 1fr))", gap: 14 }}>
        {COLS.map((c) => {
          const list = all.filter((i) => (i.status || "open") === c.key).sort((a, b) => String(b.updated_at).localeCompare(String(a.updated_at)));
          return (
            <div key={c.key} className="col" style={{ gap: 8 }}>
              <div className="row" style={{ borderBottom: `2px solid ${c.color}`, paddingBottom: 6 }}>
                <span className="label" style={{ color: c.color }}>{c.label}</span>
                <span className="mono dim" style={{ marginLeft: "auto" }}>{list.length}</span>
              </div>
              <div className="col" style={{ gap: 8, maxHeight: 560, overflow: "auto", paddingRight: 2 }}>
                {list.map((it) => {
                  const key = `${it.project_id}:${it.id}`;
                  const op = (o: Omit<ActionOp, "id">) => run(it.project_id, { ...o, id: it.id });
                  if (editing && `${editing.project_id}:${editing.id}` === key)
                    return (
                      <div key={key} className="card col" style={{ padding: "10px 12px", gap: 6, borderLeft: `3px solid ${c.color}` }}>
                        <input className="input" value={editing.title} onChange={(e) => setEditing({ ...editing, title: e.target.value })} aria-label="Title" />
                        <textarea className="input" rows={2} value={editing.notes} onChange={(e) => setEditing({ ...editing, notes: e.target.value })} aria-label="Notes" />
                        <select className="input" value={editing.assignee ?? ""} onChange={(e) => setEditing({ ...editing, assignee: e.target.value })} aria-label="Assign to">
                          <option value="">Anyone</option>
                          {people.map((p) => <option key={p.id} value={p.name}>{p.name}</option>)}
                        </select>
                        <div className="row" style={{ gap: 6 }}>
                          <button className="btn sm primary" onClick={() => op({ kind: "edit", title: editing.title, notes: editing.notes, assignee: editing.assignee ?? "" }).then((ok) => ok && setEditing(null))}>Save</button>
                          <button className="btn sm ghost" onClick={() => setEditing(null)}>Cancel</button>
                        </div>
                      </div>
                    );
                  return (
                    <div key={key} className="card" style={{ padding: "10px 12px", borderLeft: `3px solid ${c.color}` }}>
                      <div className="small" style={{ fontWeight: 600, textDecoration: it.status === "done" ? "line-through" : undefined }}>{it.title}</div>
                      {it.notes && <div className="muted small" style={{ marginTop: 4 }}>{it.notes}</div>}
                      <div className="dim small" style={{ marginTop: 6, lineHeight: 1.5 }}>
                        {!projectId && <span className="chip" style={{ marginRight: 6 }}>{it.project}</span>}
                        {it.assignee && <>for <b>{it.assignee}</b> · </>}
                        added by {it.created_by} {ago(it.created_at)}
                        {it.started_by && <> · started by {it.started_by} {ago(it.started_at)}</>}
                        {it.done_by && <> · <span style={{ color: "var(--lime)" }}>done by {it.done_by} {ago(it.done_at)}</span></>}
                      </div>
                      <div className="row wrap" style={{ marginTop: 8, gap: 6 }}>
                        {it.status === "open" && <button className="btn sm" onClick={() => op({ kind: "start" })}>Start</button>}
                        {it.status !== "done" && <button className="btn sm primary" onClick={() => op({ kind: "complete" })}>Complete</button>}
                        {it.status === "done" && <button className="btn sm" onClick={() => op({ kind: "reopen" })}>Reopen</button>}
                        <button className="btn sm ghost" onClick={() => setEditing({ ...it })}>Edit</button>
                        {confirmRemove === key
                          ? <button className="btn sm danger" onClick={() => op({ kind: "remove" }).then(() => setConfirmRemove(null))}>Confirm remove</button>
                          : <button className="btn sm ghost" onClick={() => setConfirmRemove(key)}>Remove</button>}
                      </div>
                    </div>
                  );
                })}
                {!list.length && <div className="dim small mono" style={{ padding: 8 }}>none</div>}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

export default function Actions() {
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="page-head">
        <div><h1>Action items</h1><div className="sub">Shared through each project's Book of Secrets folder · who added, started and finished every item</div></div>
      </div>
      <section className="card"><div className="card-body"><ActionBoard /></div></section>
    </div>
  );
}
