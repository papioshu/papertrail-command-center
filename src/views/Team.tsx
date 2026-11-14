import ProjectFiles from "./ProjectFiles";
import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { api, type Row } from "../api";
import { ago, AgentChip, brand, Card, Empty, ErrBox, Modal, StatusBar, useAsync, useNav } from "../ui";
import Boards from "./Boards";

/** Shared team workspaces on the self-hosted hub (D030–D035). Cards → a workspace page with its projects, tasks
 *  (claimable), media, project map and file list. Chats and local activity come from this PC only and are never sent.
 *  Buttons follow your role; the hub re-checks every edit. */

const COLS = [
  { key: "todo", label: "To do", color: "var(--violet-soft)" },
  { key: "doing", label: "In progress", color: "var(--violet)" },
  { key: "blocked", label: "Blocked", color: "var(--gold)" },
  { key: "done", label: "Done", color: "var(--lime)" },
] as const;
const RANK: Record<string, number> = { owner: 0, admin: 1, editor: 2, contributor: 3, viewer: 4 };
const can = (role: string, need: string) => (RANK[role] ?? 9) <= RANK[need];
const glowStyle = (glow: string) => ({ ["--glow" as string]: glow }) as CSSProperties;
const fmtBytes = (n: number) => (n > 1e6 ? `${(n / 1e6).toFixed(1)} MB` : n > 1e3 ? `${Math.round(n / 1e3)} KB` : `${n} B`);
const taskCounts = (tasks: Row[]) => {
  const c: Record<string, number> = { done: 0, active: 0, pending: 0, blocked: 0 };
  for (const t of tasks) { const s = t.fields?.status ?? "todo"; c[s === "doing" ? "active" : s === "todo" ? "pending" : s] = (c[s === "doing" ? "active" : s === "todo" ? "pending" : s] ?? 0) + 1; }
  return c;
};

/* ---------- not joined yet ---------- */

function Start({ host, onDone }: { host: Row | undefined; onDone: () => void }) {
  const [link, setLink] = useState("");
  const [name, setName] = useState("");
  const [wsName, setWsName] = useState("");
  const [hosting, setHosting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const go = async (f: () => Promise<unknown>) => { setBusy(true); setErr(null); try { await f(); onDone(); } catch (e) { setErr(String(e)); } finally { setBusy(false); } };
  return (
    <div className="col" style={{ gap: 14 }}>
      <Card title="Join your team">
        <div className="col" style={{ gap: 10, maxWidth: 620 }}>
          <div className="small muted">Paste the invite link you were sent (it looks like <span className="mono">http://100.x.y.z:8770#…</span>). It works once and expires. Tailscale must be on.</div>
          <input className="input" placeholder="Invite link" value={link} onChange={(e) => setLink(e.target.value)} aria-label="Invite link" />
          <input className="input" placeholder="Your name, as your team will see it" value={name} onChange={(e) => setName(e.target.value)} aria-label="Your name" />
          <div className="row"><button className="btn primary" disabled={busy || !link.trim() || !name.trim()} onClick={() => go(() => api.hubJoin(link.trim(), name.trim()))}>{busy ? "Joining…" : "Join"}</button></div>
        </div>
      </Card>
      {err && <ErrBox error={err} />}
      {host?.installed && (hosting ? (
        <Card title="Host a workspace on this PC">
          <div className="col" style={{ gap: 10, maxWidth: 620 }}>
            <div className="small muted">The team hub runs on this PC. Name a workspace (e.g. BOMBSQUAD); you'll own it and decide who's in it.</div>
            <input className="input" placeholder="Workspace name" value={wsName} onChange={(e) => setWsName(e.target.value)} aria-label="Workspace name" />
            <input className="input" placeholder="Your name, as your team will see it" value={name} onChange={(e) => setName(e.target.value)} aria-label="Owner name" />
            <div className="row" style={{ gap: 6 }}>
              <button className="btn primary" disabled={busy || !wsName.trim() || !name.trim()} onClick={() => go(() => api.hubHostCreate(wsName.trim(), name.trim()))}>{busy ? "Creating…" : "Create workspace"}</button>
              <button className="btn ghost" onClick={() => setHosting(false)}>Cancel</button>
            </div>
          </div>
        </Card>
      ) : <div><button className="btn sm ghost" onClick={() => setHosting(true)}>Host a workspace on this PC instead</button></div>)}
      <div className="dim small">Only what you put in a team workspace is shared: tasks, media, and the file list + map of projects you choose to share. Your chats, notes and files stay on this PC.</div>
    </div>
  );
}

/* ---------- connecting teammates (owner) ---------- */

function ConnectGuide({ host, people, open, setOpen }: { host: Row | undefined; people: number; open: boolean; setOpen: (o: boolean) => void }) {
  const ok = (b: boolean) => <span style={{ color: b ? "var(--lime)" : "var(--gold)", fontWeight: 700, width: 18, display: "inline-block" }}>{b ? "✓" : "•"}</span>;
  const ts = !!host?.tailscale;
  return (
    <Card title="Connect your team" right={<button className="btn sm ghost" onClick={() => setOpen(!open)}>{open ? "Hide" : "Show steps"}</button>}>
      {!open ? <div className="small dim">{ts ? `Hub reachable on Tailscale at ${host?.tailscale}:8770` : "Tailscale is off on this PC: teammates can't reach the hub yet."} · {people} teammate{people === 1 ? "" : "s"} connected</div> : (
        <div className="col" style={{ gap: 10 }}>
          <div className="small">{ok(!!host?.running)}<b>Hub running on this PC</b> {host?.running ? "· starts with Windows" : "· not answering: restart the PC or run ptcc-hub serve"}</div>
          <div className="small">{ok(ts)}<b>This PC on Tailscale</b> {ts ? `· ${host?.tailscale}` : "· open Tailscale from the taskbar tray and sign in. The hub picks the address up within a minute."}</div>
          <div className="small">{ok(!!host?.teammates?.length)}<b>Add Profit and Rich to your Tailscale network</b> · {host?.teammates?.length
              ? `connected: ${(host.teammates as Row[]).map((t) => `${t.name} (${t.online ? "online" : "offline"})`).join(", ")}`
              : "in Tailscale's admin page choose Users › Invite users and send each an invite. They install Tailscale on their PC and sign in with it."}
            <button className="btn sm ghost" style={{ marginLeft: 8 }} onClick={() => api.openUrl("https://login.tailscale.com/admin/users")}>Open Tailscale users</button></div>
          <div className="small">{ok(!!host?.installer)}<b>Send them the Command Center installer</b> · {host?.installer ? "it's in the share folder." : "the share folder is empty for now."}
            <button className="btn sm ghost" style={{ marginLeft: 8 }} onClick={() => api.hubOpenShareFolder()}>Open share folder</button></div>
          <div className="small">{ok(people > 0)}<b>Give each person their own invite link</b> · open a workspace › ADMIN › Invite, pick their role (Profit: Ops, Rich: Artist), Make invite link, and send it privately. Links work once, for 72 hours.</div>
          <div className="small">{ok(people > 0)}<b>They join</b> · on their PC: TEAM › paste the link › their name › Join. After that, use PEOPLE below to put them into more of your workspaces or take them out.</div>
        </div>
      )}
    </Card>
  );
}

/** Already on the hub: another invite link adds that workspace to the same account. */
function JoinMore({ me, onDone, onErr }: { me: string; onDone: () => void; onErr: (e: string) => void }) {
  const [link, setLink] = useState("");
  return (
    <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
      <input className="input" style={{ flex: 1, minWidth: 280 }} placeholder="Got an invite to another workspace? Paste the link here" value={link} onChange={(e) => setLink(e.target.value)} aria-label="Another invite link" />
      <button className="btn sm" disabled={!link.trim()} onClick={() => api.hubJoin(link.trim(), me || "Me").then(() => { setLink(""); onDone(); }, (e) => onErr(String(e)))}>Join</button>
    </div>
  );
}

/** A one-time sign-in link: QR for phones, the link (with Copy) for texting it or pasting into the app on a PC. */
function LinkBox({ link, qr, note }: { link: string; qr: string; note: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="col" style={{ gap: 10, alignItems: "center" }}>
      <div className="small">{note}</div>
      <img alt="QR code for the sign-in link" style={{ width: 220, height: 220, background: "#fff", padding: 8, borderRadius: 8 }} src={`data:image/svg+xml;utf8,${encodeURIComponent(qr)}`} />
      <div className="row" style={{ gap: 6, width: "100%" }}>
        <input className="input mono" readOnly value={link} onFocus={(e) => e.target.select()} style={{ flex: 1 }} aria-label="Sign-in link" />
        <button className="btn sm primary" onClick={() => navigator.clipboard.writeText(link).then(() => setCopied(true))}>{copied ? "Copied" : "Copy"}</button>
      </div>
    </div>
  );
}

/* ---------- people × workspaces (owner only, D037) ---------- */

function People({ onErr }: { onErr: (e: string) => void }) {
  const data = useAsync(() => api.hubAdmin("", "people"), [], ["hub-changed"]);
  const owned: Row[] = data.data?.workspaces ?? [];
  const roles = useAsync(() => Promise.all(owned.map((w) => api.hubAdmin(w.id, "roles"))), [owned.map((w) => w.id).join()]);
  const [confirm, setConfirm] = useState<string | null>(null);
  const [addAs, setAddAs] = useState<Record<string, string>>({});
  const [signin, setSignin] = useState<{ name: string; link: string; qr_svg: string } | null>(null);
  if (!owned.length) return null;
  const people: Row[] = data.data?.people ?? [];
  const choices = (i: number) => [...((roles.data?.[i]?.roles ?? []) as Row[]).map((r) => ({ v: `t:${r.name}`, label: r.name })), ...["admin", "editor", "contributor", "viewer"].map((r) => ({ v: `r:${r}`, label: r }))];
  const body = (v: string, uid: string) => (v.startsWith("t:") ? { user_id: uid, title: v.slice(2) } : { user_id: uid, role: v.slice(2) });
  const run = (ws: string, action: string, b: Row) => api.hubAdmin(ws, action, b).then(() => { setConfirm(null); data.reload(); }, (e) => onErr(String(e)));
  return (
    <Card title="People">
      {!people.length ? <Empty>Nobody else has joined yet. Use "Connect your team" above to invite people; once they've joined one workspace you can put them in the others here.</Empty> : (
        <div style={{ overflowX: "auto" }}>
          <table className="table" style={{ width: "100%" }}>
            <thead><tr><th style={{ textAlign: "left" }}>Person</th>{owned.map((w) => <th key={w.id} style={{ textAlign: "left" }}>{w.name}</th>)}</tr></thead>
            <tbody>
              {people.map((p) => (
                <tr key={p.user_id}>
                  <td><b>{p.name}</b>
                    <button className="btn sm ghost" style={{ marginLeft: 8 }} title={`A one-time link that signs ${p.name}'s phone, iPad or PC app into their account`}
                      onClick={() => api.hubSigninLink(p.user_id).then((r) => setSignin({ name: p.name, link: String(r.link), qr_svg: String(r.qr_svg) }), (e) => onErr(String(e)))}>Sign-in link</button></td>
                  {owned.map((w, i) => {
                    const m = p.workspaces?.[w.id];
                    const key = `${p.user_id}|${w.id}`;
                    if (m) {
                      const cur = m.title ? `t:${m.title}` : `r:${m.role}`;
                      return (
                        <td key={w.id}>
                          <div className="row" style={{ gap: 6 }}>
                            <select className="input" value={cur} onChange={(e) => run(w.id, "member_set", body(e.target.value, p.user_id))} aria-label={`${p.name} in ${w.name}`}>
                              {!choices(i).some((c) => c.v === cur) && <option value={cur}>{m.title ?? m.role}</option>}
                              {choices(i).map((c) => <option key={c.v} value={c.v}>{c.label}</option>)}
                            </select>
                            {confirm === key
                              ? <button className="btn sm danger" onClick={() => run(w.id, "member_remove", { user_id: p.user_id })}>Confirm remove</button>
                              : <button className="btn sm ghost" onClick={() => setConfirm(key)} aria-label={`Remove ${p.name} from ${w.name}`}>Remove</button>}
                          </div>
                        </td>
                      );
                    }
                    const as = addAs[key] ?? choices(i)[0]?.v ?? "r:viewer";
                    return (
                      <td key={w.id}>
                        <div className="row" style={{ gap: 6 }}>
                          <select className="input" value={as} onChange={(e) => setAddAs({ ...addAs, [key]: e.target.value })} aria-label={`Role for ${p.name} in ${w.name}`}>
                            {choices(i).map((c) => <option key={c.v} value={c.v}>{c.label}</option>)}
                          </select>
                          <button className="btn sm" onClick={() => run(w.id, "member_add", body(as, p.user_id))} aria-label={`Add ${p.name} to ${w.name}`}>Add</button>
                        </div>
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <div className="dim small" style={{ marginTop: 8 }}>Only you, as owner, can put people into your workspaces or take them out. Admins can change roles but not who's in. Sign-in link: for a new or signed-out device of someone already in; it opens their existing account (no new invite).</div>
      {signin && (
        <Modal title={`Sign-in link for ${signin.name}`} onClose={() => setSignin(null)} foot={<button className="btn primary" onClick={() => setSignin(null)}>Done</button>}>
          <LinkBox link={signin.link} qr={signin.qr_svg} note={`Send it to ${signin.name} privately: it signs whoever opens it in as ${signin.name}. iPhone/iPad: open it in Safari (or scan the QR). PC: paste it into the Command Center app under TEAM. Works once, for 24 hours. Tailscale must be on there.`} />
        </Modal>
      )}
    </Card>
  );
}

/* ---------- workspace cards ---------- */

function WsCard({ w, onOpen }: { w: Row; onOpen: () => void }) {
  const board = useAsync(() => api.hubBoard(w.id), [w.id], ["hub-changed"]);
  const b = board.data;
  const b2 = brand(w.name);
  const tasks: Row[] = (b?.records ?? []).filter((r: Row) => r.kind === "task");
  const open = tasks.filter((t) => t.fields?.status !== "done");
  const unclaimed = open.filter((t) => !t.fields?.assignee).length;
  const last = (b?.records ?? []).map((r: Row) => r.updated_at).filter(Boolean).sort().pop();
  return (
    <button className={`acard ${b2.img ? "" : "gen"}`} style={glowStyle(b2.glow)} onClick={onOpen} title={w.name}>
      {b2.img ? <img src={b2.img} alt={w.name} /> : <div className="title">{w.name}</div>}
      {(b?.conflicts?.length ?? 0) > 0 && <span className="chip warn flag">{b?.conflicts?.length} need you</span>}
      <div className="shade" />
      {tasks.length > 0 && <div style={{ position: "absolute", left: 14, right: 14, bottom: 46 }}><StatusBar counts={taskCounts(tasks)} height={4} /></div>}
      <div className="meta">
        <span className="k">{(b?.projects ?? []).length} proj · {(b?.members ?? []).length} ppl · {open.length} open{unclaimed ? ` · ${unclaimed} unclaimed` : ""}</span>
        <span className="v">{w.role} · {last ? ago(last) : "new"}</span>
      </div>
      <span className="arrow">→</span>
    </button>
  );
}

function NewWsCard({ me, onDone, onErr }: { me: string; onDone: () => void; onErr: (e: string) => void }) {
  const [name, setName] = useState("");
  const [open, setOpen] = useState(false);
  if (!open) return <button className="acard gen" style={glowStyle("#8b5cf6")} onClick={() => setOpen(true)}><div className="title">+ NEW WORKSPACE</div><div className="shade" /></button>;
  return (
    <div className="card col" style={{ padding: 14, gap: 8, justifyContent: "center" }}>
      <span className="label">New workspace</span>
      <input className="input" autoFocus placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} aria-label="New workspace name" />
      <div className="row" style={{ gap: 6 }}>
        <button className="btn sm primary" disabled={!name.trim()} onClick={() => api.hubHostCreate(name.trim(), me).then(() => { setOpen(false); setName(""); onDone(); }, (e) => onErr(String(e)))}>Create</button>
        <button className="btn sm ghost" onClick={() => setOpen(false)}>Cancel</button>
      </div>
    </div>
  );
}

/* ---------- tasks ---------- */

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
              <option value="">Unclaimed</option>
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
        {f.assignee ? <>claimed by <b style={{ color: f.assignee === me ? "var(--lime)" : undefined }}>{f.assignee === me ? "you" : name(f.assignee)}</b> · </> : <span style={{ color: "var(--gold)" }}>unclaimed · </span>}
        {f.due && <>due {f.due} · </>}
        {t.created_by ? <>by {name(t.created_by)}</> : <>new</>}
        {t.updated_at && <> · {ago(t.updated_at)}</>}
        {t.pending > 0 && <> · <span style={{ color: "var(--gold)" }}>waiting to sync</span></>}
      </div>
      <div className="row wrap" style={{ marginTop: 8, gap: 6 }}>
        {can(role, "contributor") && !f.assignee && <button className="btn sm primary" onClick={() => run("update", { assignee: me })}>Claim</button>}
        {f.assignee === me && <button className="btn sm ghost" onClick={() => run("update", { assignee: null })}>Unclaim</button>}
        {editable && COLS.filter((c) => c.key !== (f.status ?? "todo")).map((c) => <button key={c.key} className="btn sm" onClick={() => run("update", { status: c.key })}>{c.label}</button>)}
        {editable && <button className="btn sm ghost" onClick={() => setEditing({ ...f })}>Edit</button>}
        {editable && (confirm ? <button className="btn sm danger" onClick={() => run("delete", null)}>Confirm delete</button> : <button className="btn sm ghost" onClick={() => setConfirm(true)}>Delete</button>)}
      </div>
    </div>
  );
}

function Tasks({ ws, b, project, me, onErr, reload }: { ws: string; b: Row; project: string; me: string; onErr: (e: string) => void; reload: () => void }) {
  const role: string = b.role ?? "viewer";
  const members: Row[] = b.members ?? [];
  const [draft, setDraft] = useState({ title: "", assignee: "", due: "", project: "" });
  const [mineOnly, setMineOnly] = useState(false);
  const target = project || draft.project || b.projects?.[0]?.id || "";
  const pname = (id: string) => b.projects?.find((p: Row) => p.id === id)?.name ?? "";
  const tasks: Row[] = (b.records ?? []).filter((r: Row) => r.kind === "task" && (!project || r.project_id === project) && (!mineOnly || r.fields?.assignee === me));
  const add = () => {
    const fields: Row = { title: draft.title.trim(), status: "todo" };
    if (draft.assignee) fields.assignee = draft.assignee;
    if (draft.due) fields.due = draft.due;
    api.hubEdit(ws, "create", "task", target, fields).then(() => { setDraft({ ...draft, title: "", assignee: "", due: "" }); reload(); }, (e) => onErr(String(e)));
  };
  return (
    <div className="col" style={{ gap: 12 }}>
      {can(role, "contributor") && target && (
        <div className="row wrap" style={{ gap: 8 }}>
          {!project && (
            <select className="input" value={target} onChange={(e) => setDraft({ ...draft, project: e.target.value })} aria-label="Project for the new task">
              {(b.projects ?? []).map((p: Row) => <option key={p.id} value={p.id}>{p.name}</option>)}
            </select>
          )}
          <input className="input" style={{ flex: 2, minWidth: 240 }} placeholder="New shared task…" value={draft.title} onChange={(e) => setDraft({ ...draft, title: e.target.value })} onKeyDown={(e) => e.key === "Enter" && draft.title.trim() && add()} aria-label="Task title" />
          <select className="input" value={draft.assignee} onChange={(e) => setDraft({ ...draft, assignee: e.target.value })} aria-label="Claimed by">
            <option value="">Unclaimed</option>
            {(can(role, "editor") ? members : members.filter((m) => m.user_id === me)).map((m) => <option key={m.user_id} value={m.user_id}>{m.user_id === me ? `Me (${m.name})` : m.name}</option>)}
          </select>
          <input className="input" type="date" value={draft.due} onChange={(e) => setDraft({ ...draft, due: e.target.value })} aria-label="Due date" />
          <button className="btn primary" disabled={!draft.title.trim()} onClick={add}>Add</button>
        </div>
      )}
      <label className="row small dim" style={{ gap: 6 }}><input type="checkbox" checked={mineOnly} onChange={(e) => setMineOnly(e.target.checked)} /> Only tasks I've claimed</label>
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
                {list.map((t) => (
                  <div key={t.id}>
                    {!project && <div className="dim small mono" style={{ margin: "0 0 2px 4px" }}>{pname(t.project_id)}</div>}
                    <TaskCard t={t} ws={ws} me={me} role={role} members={members} color={c.color} onErr={onErr} reload={reload} />
                  </div>
                ))}
                {!list.length && <div className="dim small mono" style={{ padding: 8 }}>none</div>}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

/* ---------- media ---------- */

function Thumb({ ws, r }: { ws: string; r: Row }) {
  const name = String(r.fields?.name ?? "file");
  const src = useAsync(() => api.hubFile(ws, r.id, name) as Promise<string>, [ws, r.id]);
  const kind = /\.(png|jpe?g|gif|webp|svg)$/i.test(name) ? "img" : /\.(mp4|webm)$/i.test(name) ? "video" : "file";
  return (
    <div className="card" style={{ padding: 8 }}>
      <div style={{ height: 150, display: "grid", placeItems: "center", background: "rgba(0,0,0,.25)", borderRadius: 6, overflow: "hidden" }}>
        {src.error ? <span className="dim small">{src.error}</span> : !src.data ? <span className="dim small">loading…</span>
          : kind === "img" ? <img src={src.data} alt={name} style={{ maxWidth: "100%", maxHeight: 150, objectFit: "contain" }} />
          : kind === "video" ? <video src={src.data} controls style={{ maxWidth: "100%", maxHeight: 150 }} />
          : <a className="btn sm" href={src.data} download={name}>Download</a>}
      </div>
      <div className="small" style={{ marginTop: 6, fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={name}>{name}</div>
      <div className="dim small">{r.fields?.folder ? `${r.fields.folder} · ` : ""}{r.fields?.caption ? `${r.fields.caption} · ` : ""}{fmtBytes(Number(r.fields?.size ?? 0))} · {ago(r.updated_at)}</div>
    </div>
  );
}

function Media({ ws, b, project, onErr, reload }: { ws: string; b: Row; project: string; onErr: (e: string) => void; reload: () => void }) {
  const items: Row[] = (b.records ?? []).filter((r: Row) => r.kind === "media" && (!project || r.project_id === project));
  const [busy, setBusy] = useState(false);
  const [caption, setCaption] = useState("");
  const [folder, setFolder] = useState("");
  const target = project || b.projects?.[0]?.id || "";
  const folders = [...new Set(items.map((r) => r.fields?.folder).filter(Boolean))] as string[];
  const upload = (file: File | undefined) => {
    if (!file || !target) return;
    if (file.size > 25 * 1024 * 1024) return onErr("Files can be at most 25 MB.");
    setBusy(true);
    const rd = new FileReader();
    rd.onload = () => api.hubUpload(ws, target, file.name, caption, folder, String(rd.result)).then(() => { setCaption(""); reload(); }, (e) => onErr(String(e))).finally(() => setBusy(false));
    rd.onerror = () => { setBusy(false); onErr("Couldn't read that file."); };
    rd.readAsDataURL(file);
  };
  return (
    <div className="col" style={{ gap: 12 }}>
      {can(b.role, "contributor") && target && (
        <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
          <input className="input" list="media-folders" placeholder="Folder (e.g. Renders)" value={folder} onChange={(e) => setFolder(e.target.value)} aria-label="Folder" />
          <datalist id="media-folders">{folders.map((f) => <option key={f} value={f} />)}</datalist>
          <input className="input" placeholder="Caption (optional)" value={caption} onChange={(e) => setCaption(e.target.value)} aria-label="Caption" />
          <label className="btn sm primary" style={{ cursor: busy ? "wait" : "pointer" }}>{busy ? "Uploading…" : `Upload to ${b.projects?.find((p: Row) => p.id === target)?.name ?? "project"}`}
            <input type="file" hidden disabled={busy} onChange={(e) => upload(e.target.files?.[0])} /></label>
          <span className="dim small">Images, video, audio, PDFs, up to 25 MB. The hub needs to be reachable.</span>
        </div>
      )}
      {!items.length ? <Empty>No media here yet.</Empty> : (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(200px, 1fr))", gap: 12 }}>
          {items.map((r) => <Thumb key={r.id} ws={ws} r={r} />)}
        </div>
      )}
    </div>
  );
}

/* ---------- project map + files (from the shared snapshot) ---------- */

function useSnapshot(ws: string, b: Row, project: string) {
  const rec: Row | undefined = (b.records ?? []).filter((r: Row) => r.kind === "snapshot" && r.project_id === project).sort((x: Row, y: Row) => String(y.updated_at).localeCompare(String(x.updated_at)))[0];
  const snap = useAsync(() => (rec ? (api.hubFile(ws, rec.id) as Promise<Row>) : Promise.resolve(null)), [ws, rec?.id]);
  return { rec, snap };
}

/** Radial "neural" map of a project's folders: rings by depth, node size by file count. */
function NeuralMap({ snap, glow, onPick, picked }: { snap: Row; glow: string; onPick: (d: string) => void; picked: string }) {
  const layout = useMemo(() => {
    const nodes: Row[] = (snap.map?.nodes ?? []).filter((n: Row) => String(n.id).split("/").length <= 4 || n.id === "");
    const kids = new Map<string, Row[]>();
    for (const n of nodes) if (n.id !== "") { const p = String(n.id).includes("/") ? String(n.id).slice(0, String(n.id).lastIndexOf("/")) : ""; kids.set(p, [...(kids.get(p) ?? []), n]); }
    const leaves = (id: string): number => { const k = kids.get(id) ?? []; return k.length ? k.reduce((a, c) => a + leaves(c.id), 0) : 1; };
    const pos = new Map<string, { x: number; y: number; n: Row; depth: number }>();
    const place = (id: string, a0: number, a1: number, depth: number) => {
      const n = nodes.find((x) => x.id === id); if (!n) return;
      const a = (a0 + a1) / 2, r = depth * 95;
      pos.set(id, { x: r * Math.cos(a), y: r * Math.sin(a), n, depth });
      let s = a0; const tot = leaves(id);
      for (const c of (kids.get(id) ?? []).sort((p, q) => q.files - p.files)) { const w = ((a1 - a0) * leaves(c.id)) / tot; place(c.id, s, s + w, depth + 1); s += w; }
    };
    place("", 0, Math.PI * 2, 0);
    return pos;
  }, [snap]);
  const all = [...layout.values()];
  const max = Math.max(1, ...all.map((p) => p.depth));
  const R = (max + 0.6) * 95;
  return (
    <svg viewBox={`${-R} ${-R} ${2 * R} ${2 * R}`} style={{ width: "100%", maxHeight: 620, display: "block" }} role="img" aria-label="Project folder map">
      {[...Array(max)].map((_, i) => <circle key={i} r={(i + 1) * 95} fill="none" stroke={glow} strokeOpacity={0.08} />)}
      {all.filter((p) => p.n.id !== "").map((p) => {
        const parent = layout.get(String(p.n.id).includes("/") ? String(p.n.id).slice(0, String(p.n.id).lastIndexOf("/")) : "");
        return parent && <line key={`l${p.n.id}`} x1={parent.x} y1={parent.y} x2={p.x} y2={p.y} stroke={glow} strokeOpacity={0.28} />;
      })}
      {all.map((p) => {
        const r = Math.min(22, 3 + Math.sqrt(Number(p.n.files)) * 1.1);
        const on = picked === p.n.id;
        return (
          <g key={`n${p.n.id}`} style={{ cursor: "pointer" }} onClick={() => onPick(String(p.n.id))}>
            <circle cx={p.x} cy={p.y} r={r + 5} fill={glow} fillOpacity={on ? 0.35 : 0.08} />
            <circle cx={p.x} cy={p.y} r={r} fill={on ? "var(--lime)" : glow} fillOpacity={0.85 - p.depth * 0.12} />
            {p.depth <= 2 && <text x={p.x} y={p.y + r + 12} fontSize={11} textAnchor="middle" fill="var(--fg)" opacity={0.85}>{p.n.id === "" ? "root" : String(p.n.id).split("/").pop()}</text>}
            <title>{`${p.n.id || "root"} · ${p.n.files} files · ${fmtBytes(Number(p.n.bytes))}`}</title>
          </g>
        );
      })}
    </svg>
  );
}

function Files({ snap, dir, setDir }: { snap: Row; dir: string; setDir: (d: string) => void }) {
  const [q, setQ] = useState("");
  const files: [string, number][] = snap.files ?? [];
  const prefix = dir ? dir + "/" : "";
  const inDir = files.filter(([p]) => p.startsWith(prefix) && (!q || p.toLowerCase().includes(q.toLowerCase())));
  const sub = new Map<string, number>();
  const here: [string, number][] = [];
  for (const [p, s] of inDir) { const rest = p.slice(prefix.length); const i = rest.indexOf("/"); if (i < 0 || q) here.push([p, s]); else sub.set(rest.slice(0, i), (sub.get(rest.slice(0, i)) ?? 0) + 1); }
  const crumbs = dir ? dir.split("/") : [];
  return (
    <div className="col" style={{ gap: 8 }}>
      <div className="row wrap" style={{ gap: 6, alignItems: "center" }}>
        <button className="btn sm ghost" onClick={() => setDir("")}>root</button>
        {crumbs.map((c, i) => <span key={i} className="row" style={{ gap: 6 }}><span className="dim">/</span><button className="btn sm ghost" onClick={() => setDir(crumbs.slice(0, i + 1).join("/"))}>{c}</button></span>)}
        <input className="input" style={{ marginLeft: "auto", width: 240 }} placeholder="Find a file…" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Find a file" />
      </div>
      <div className="col" style={{ maxHeight: 520, overflow: "auto" }}>
        {[...sub.entries()].sort().map(([d, n]) => <div key={d} className="li click" onClick={() => setDir(prefix + d)}><span>📁 {d}</span><span className="dim small mono" style={{ marginLeft: "auto" }}>{n} files</span></div>)}
        {here.slice(0, 1000).map(([p, s]) => <div key={p} className="li"><span className="mono small">{q ? p : p.slice(prefix.length)}</span><span className="dim small mono" style={{ marginLeft: "auto" }}>{fmtBytes(s)}</span></div>)}
        {!sub.size && !here.length && <div className="dim small">No files match.</div>}
        {here.length > 1000 && <div className="dim small">Showing the first 1000 of {here.length}.</div>}
      </div>
    </div>
  );
}

function Structure({ ws, b, project, glow, mode, local, me, onErr, reload }: { ws: string; b: Row; project: string; glow: string; mode: "map" | "files"; local?: string; me: string; onErr: (e: string) => void; reload: () => void }) {
  const { rec, snap } = useSnapshot(ws, b, project);
  const [dir, setDir] = useState("");
  const [preview, setPreview] = useState<Row | null>(null);
  if (!project) return <Empty>Pick a project above to see its {mode === "map" ? "map" : "files"}.</Empty>;
  if (mode === "files") return <ProjectFiles ws={ws} b={b} project={project} local={local} me={me} onErr={onErr} reload={reload} />;
  const publish = local && can(b.role, "editor");
  return (
    <div className="col" style={{ gap: 10 }}>
      <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
        <span className="dim small">{rec ? `Shared file list from ${ago(rec.updated_at)} · ${rec.fields?.files} files · ${rec.fields?.dirs} folders` : "No file list shared for this project yet."}</span>
        {publish && <button className="btn sm" style={{ marginLeft: "auto" }} onClick={() => api.hubSnapshotPreview(local!).then(setPreview, (e) => onErr(String(e)))}>{rec ? "Share an updated file list" : "Share file list + map"}</button>}
      </div>
      {snap.error && <ErrBox error={snap.error} />}
      {snap.data && (mode === "map"
        ? <NeuralMap snap={snap.data} glow={glow} picked={dir} onPick={setDir} />
        : <Files snap={snap.data} dir={dir} setDir={setDir} />)}
      {mode === "map" && snap.data && dir && <div className="dim small">Selected <span className="mono">{dir}</span>: open the FILES tab to browse it.</div>}
      {preview && (
        <Modal title="Share this project's file list + map" onClose={() => setPreview(null)}
          foot={<><button className="btn ghost" onClick={() => setPreview(null)}>Cancel</button>
            <button className="btn primary" onClick={() => api.hubSnapshotPublish(local!).then(() => { setPreview(null); reload(); }, (e) => onErr(String(e)))}>Share</button></>}>
          <div className="col" style={{ gap: 6 }}>
            <div className="small"><b>{preview.files}</b> files in <b>{preview.dirs}</b> folders. Top level: {(preview.top as Row[]).map((t) => t.id).join(", ") || "(files only)"}</div>
            <div className="small">Sent: {(preview.sent as string[]).join("; ")}.</div>
            <div className="small">Never sent: {(preview.never as string[]).join("; ")}.</div>
            {preview.excluded > 0 && <div className="small" style={{ color: "var(--gold)" }}>{preview.excluded} secret-looking file{preview.excluded === 1 ? " is" : "s are"} left out.</div>}
          </div>
        </Modal>
      )}
    </div>
  );
}

/* ---------- local-only: chats and activity for projects shared from this PC ---------- */

function LocalChats({ local }: { local?: string }) {
  const { nav } = useNav();
  const list = useAsync(() => (local ? api.chatList(local) : Promise.resolve([])), [local]);
  if (!local) return <Empty>Chats show here for projects shared from this PC. They stay on this PC and are never sent to the team.</Empty>;
  return (
    <div className="col" style={{ gap: 4 }}>
      <div className="dim small">Only you see these (from this PC). They are never sent to the team.</div>
      {(list.data ?? []).slice(0, 200).map((c) => (
        <div key={c.id} className="li click" onClick={() => nav("chat", { id: c.id })}>
          <AgentChip agent={c.agent} /><span className="small">{c.title || "(untitled)"}</span><span className="dim small" style={{ marginLeft: "auto" }}>{ago(c.updated)}</span>
        </div>
      ))}
      {list.data && !list.data.length && <Empty>No chats for this project on this PC.</Empty>}
    </div>
  );
}

function ProjectTile({ p, b, local, glow, onOpen }: { p: Row; b: Row; local?: string; glow: string; onOpen: () => void }) {
  const tasks: Row[] = (b.records ?? []).filter((r: Row) => r.kind === "task" && r.project_id === p.id);
  const media = (b.records ?? []).filter((r: Row) => r.kind === "media" && r.project_id === p.id).length;
  const snap = (b.records ?? []).find((r: Row) => r.kind === "snapshot" && r.project_id === p.id);
  const lp = useAsync(() => (local ? api.project(local) : Promise.resolve(null)), [local]);
  const people = new Set(tasks.map((t) => t.fields?.assignee).filter(Boolean)).size;
  return (
    <div className="card click" style={{ padding: 14, ...glowStyle(glow), borderTop: `2px solid ${glow}`, cursor: "pointer" }} onClick={onOpen}>
      <div className="row" style={{ alignItems: "baseline" }}><b>{p.name}</b>{local && <span className="chip" style={{ marginLeft: 8 }} title="Shared from this PC">this PC</span>}</div>
      <div style={{ margin: "8px 0" }}>{tasks.length > 0 ? <StatusBar counts={taskCounts(tasks)} height={5} /> : <div className="dim small">No tasks yet</div>}</div>
      <div className="dim small">{tasks.filter((t) => t.fields?.status !== "done").length} open · {tasks.filter((t) => t.fields?.status !== "done" && !t.fields?.assignee).length} unclaimed · {people} people on it · {media} media{snap ? ` · ${snap.fields?.files} files` : ""}</div>
      {lp.data?.project && <div className="muted small" style={{ marginTop: 6 }}>{lp.data.project.next_action ? `Next: ${lp.data.project.next_action}` : lp.data.project.summary ?? ""}{lp.data.project.last_activity ? ` · active ${ago(lp.data.project.last_activity)}` : ""}</div>}
    </div>
  );
}

/* ---------- admin: members, invites, roles (D036), projects ---------- */

const PERM_LABEL: Record<string, string> = {
  "workspace.read": "See the workspace", "project.read": "See projects", "task.read": "See tasks", "doc.read": "See file lists + maps", "media.read": "See media",
  "task.create": "Add + claim tasks", "task.edit_own": "Edit own tasks", "media.upload": "Upload media", "task.edit_any": "Edit anyone's tasks",
  "task.assign": "Assign people", "milestone.edit": "Edit milestones", "doc.publish": "Share file lists + maps", "project.link": "Share projects",
  "members.invite": "Invite people", "audit.read": "Read the audit log", "members.manage": "Change roles + remove people", "workspace.manage": "Manage roles + workspace",
  "workspace.delete": "Delete the workspace", "ownership.transfer": "Hand over ownership",
};
const roleLabel = (m: Row) => m.title ?? m.role;

/** Create or edit a named role: a base level plus permissions added or taken away. */
function RoleEditor({ ws, meta, myRole, init, onDone, onErr }: { ws: string; meta: Row; myRole: string; init: Row | null; onDone: () => void; onErr: (e: string) => void }) {
  const bases = ["admin", "editor", "contributor", "viewer"].filter((r) => RANK[r] > RANK[myRole]);
  const [name, setName] = useState(init?.name ?? "");
  const [base, setBase] = useState<string>(init?.base ?? bases[bases.length - 2] ?? "viewer");
  const [grant, setGrant] = useState<string[]>(init?.grant ?? []);
  const [deny, setDeny] = useState<string[]>(init?.deny ?? []);
  const has: string[] = meta.bases?.[base] ?? [];
  const perms: Row[] = meta.permissions ?? [];
  const toggle = (list: string[], set: (l: string[]) => void, p: string) => set(list.includes(p) ? list.filter((x) => x !== p) : [...list, p]);
  const save = () => api.hubAdmin(ws, "role_save", { name: name.trim(), base, grant: grant.filter((p) => !has.includes(p)), deny: deny.filter((p) => has.includes(p)) }).then(onDone, (e) => onErr(String(e)));
  return (
    <div className="card col" style={{ padding: 12, gap: 8 }}>
      <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
        <input className="input" placeholder="Role name (e.g. Artist)" value={name} disabled={!!init} onChange={(e) => setName(e.target.value)} aria-label="Role name" />
        <span className="small dim">built on</span>
        <select className="input" value={base} onChange={(e) => setBase(e.target.value)} aria-label="Base level">{bases.map((b) => <option key={b} value={b}>{b}</option>)}</select>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(240px, 1fr))", gap: 4 }}>
        {perms.map((p) => {
          const inBase = has.includes(p.name);
          const on = inBase ? !deny.includes(p.name) : grant.includes(p.name);
          const locked = !inBase && !p.grantable;
          return (
            <label key={p.name} className="row small" style={{ gap: 6, opacity: locked ? 0.4 : 1 }} title={locked ? "Comes only with a higher base level" : p.name}>
              <input type="checkbox" checked={on} disabled={locked} onChange={() => (inBase ? toggle(deny, setDeny, p.name) : toggle(grant, setGrant, p.name))} />
              {PERM_LABEL[p.name] ?? p.name}{!inBase && on ? " (added)" : inBase && !on ? " (removed)" : ""}
            </label>
          );
        })}
      </div>
      <div className="row" style={{ gap: 6 }}>
        <button className="btn sm primary" disabled={!name.trim()} onClick={save}>{init ? "Save role" : "Create role"}</button>
        <button className="btn sm ghost" onClick={onDone}>Cancel</button>
      </div>
    </div>
  );
}

function Admin({ ws, b, me, reload, onErr }: { ws: string; b: Row; me: string; reload: () => void; onErr: (e: string) => void }) {
  const meta = useAsync(() => api.hubAdmin(ws, "roles"), [ws], ["hub-changed"]);
  const [inviteAs, setInviteAs] = useState("");
  const [link, setLink] = useState<string | null>(null);
  const [pname, setPname] = useState("");
  const [local, setLocal] = useState("");
  const [preview, setPreview] = useState<Row | null>(null);
  const [doImport, setDoImport] = useState(true);
  const [editing, setEditing] = useState<Row | "new" | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);
  const projects = useAsync(() => api.projects(), []);
  const linked = new Set(Object.values(b.links ?? {}) as string[]);
  const myRole: string = b.role;
  const below = (r: string) => RANK[r] > RANK[myRole];
  const named: Row[] = (meta.data?.roles ?? []).filter((r: Row) => below(r.base));
  const builtins = ["admin", "editor", "contributor", "viewer"].filter(below);
  const choices = [...named.map((r) => ({ v: `t:${r.name}`, label: `${r.name} (${r.base})` })), ...builtins.map((r) => ({ v: `r:${r}`, label: r }))];
  const as = inviteAs || choices[0]?.v || "";
  const refresh = () => { meta.reload(); reload(); };
  const setMember = (uid: string, v: string) =>
    api.hubAdmin(ws, "member_set", v.startsWith("t:") ? { user_id: uid, title: v.slice(2) } : { user_id: uid, role: v.slice(2) }).then(refresh, (e) => onErr(String(e)));
  return (
    <div className="col" style={{ gap: 16 }}>
      <section className="col" style={{ gap: 8 }}>
        <span className="label">Members</span>
        {(b.members ?? []).map((m: Row) => {
          const manageable = m.user_id !== me && below(m.role) && can(myRole, "admin");
          const owner = myRole === "owner";
          const cur = m.title ? `t:${m.title}` : `r:${m.role}`;
          return (
            <div key={m.user_id} className="li">
              <b>{m.name}</b>{m.user_id === me && <span className="dim small">you</span>}
              <span className="chip" style={{ flex: "0 0 auto" }}>{roleLabel(m)}</span>
              {manageable && (
                <span className="row" style={{ marginLeft: "auto", gap: 6 }}>
                  <select className="input" value={cur} onChange={(e) => setMember(m.user_id, e.target.value)} aria-label={`Role for ${m.name}`}>
                    {!choices.some((c) => c.v === cur) && <option value={cur}>{roleLabel(m)}</option>}
                    {choices.map((c) => <option key={c.v} value={c.v}>{c.label}</option>)}
                  </select>
                  {owner && (removing === m.user_id
                    ? <button className="btn sm danger" onClick={() => api.hubAdmin(ws, "member_remove", { user_id: m.user_id }).then(() => { setRemoving(null); refresh(); }, (e) => onErr(String(e)))}>Confirm remove</button>
                    : <button className="btn sm ghost" onClick={() => setRemoving(m.user_id)}>Remove</button>)}
                </span>
              )}
            </div>
          );
        })}
      </section>
      {myRole === "owner" && <section className="col" style={{ gap: 8 }}>
        <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
          <span className="label">Invite</span>
          <select className="input" value={as} onChange={(e) => setInviteAs(e.target.value)} aria-label="Invite as">{choices.map((c) => <option key={c.v} value={c.v}>{c.label}</option>)}</select>
          <button className="btn sm" disabled={!as} onClick={() => api.hubInvite(ws, as.slice(2)).then(setLink, (e) => onErr(String(e)))}>Make invite link</button>
          {link && <><input className="input mono" style={{ flex: 1, minWidth: 260 }} readOnly value={link} onFocus={(e) => e.target.select()} aria-label="Invite link" />
            <button className="btn sm" onClick={() => navigator.clipboard.writeText(link)}>Copy</button></>}
        </div>
        {link && <div className="dim small">Works once, for 72 hours. Send it privately: whoever opens it first joins as {as.slice(2)}.{link.includes("://127.0.0.1") ? " Tailscale is off on this PC, so this link only works here; sign into Tailscale first." : ""}</div>}
      </section>}
      <section className="col" style={{ gap: 8 }}>
        <div className="row" style={{ gap: 8, alignItems: "center" }}>
          <span className="label">Roles</span>
          {can(myRole, "admin") && !editing && <button className="btn sm" style={{ marginLeft: "auto" }} onClick={() => setEditing("new")}>New role</button>}
        </div>
        {meta.error && <ErrBox error={meta.error} />}
        {editing && meta.data && <RoleEditor key={editing === "new" ? "new" : editing.name} ws={ws} meta={meta.data} myRole={myRole} init={editing === "new" ? null : editing} onErr={onErr} onDone={() => { setEditing(null); refresh(); }} />}
        {(meta.data?.roles ?? []).map((r: Row) => (
          <div key={r.name} className="li" style={{ alignItems: "flex-start" }}>
            <div style={{ flex: 1 }}>
              <b>{r.name}</b> <span className="dim small">· built on {r.base} · {r.members} member{r.members === 1 ? "" : "s"}</span>
              <div className="dim small">
                {(r.grant as string[]).length > 0 && <>adds: {(r.grant as string[]).map((p) => PERM_LABEL[p] ?? p).join(", ")}. </>}
                {(r.deny as string[]).length > 0 && <>removes: {(r.deny as string[]).map((p) => PERM_LABEL[p] ?? p).join(", ")}.</>}
              </div>
            </div>
            {can(myRole, "admin") && below(r.base) && <span className="row" style={{ gap: 6, marginLeft: "auto" }}>
              <button className="btn sm ghost" onClick={() => setEditing(r)}>Edit</button>
              <button className="btn sm ghost" onClick={() => api.hubAdmin(ws, "role_delete", { name: r.name }).then(refresh, (e) => onErr(String(e)))}>Delete</button>
            </span>}
          </div>
        ))}
        <div className="dim small">Built-in levels: owner · admin · editor · contributor · viewer. A role's people move with it when you change it.</div>
      </section>
      <section className="col" style={{ gap: 8 }}>
        <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
          <span className="label">New shared project</span>
          <input className="input" placeholder="Name" value={pname} onChange={(e) => setPname(e.target.value)} aria-label="Project name" />
          <button className="btn sm" disabled={!pname.trim()} onClick={() => api.hubNewProject(ws, pname.trim()).then(() => { setPname(""); reload(); }, (e) => onErr(String(e)))}>Create</button>
        </div>
        <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
          <span className="label">Share a project from this PC</span>
          <select className="input" value={local} onChange={(e) => setLocal(e.target.value)} aria-label="Local project">
            <option value="">Pick a project…</option>
            {(projects.data ?? []).filter((p) => !linked.has(p.id)).map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
          </select>
          <button className="btn sm" disabled={!local} onClick={() => api.hubLinkPreview(local).then(setPreview, (e) => onErr(String(e)))}>Preview what's shared</button>
        </div>
      </section>
      {preview && (
        <Modal title={`Share "${preview.project_name}" with the team`} onClose={() => setPreview(null)}
          foot={<><button className="btn ghost" onClick={() => setPreview(null)}>Cancel</button>
            <button className="btn primary" onClick={() => api.hubLink(local, ws, doImport).then(() => { setPreview(null); setLocal(""); reload(); }, (e) => onErr(String(e)))}>Share</button></>}>
          <div className="col" style={{ gap: 8 }}>
            <div className="small">Exactly this goes to the hub now:</div>
            <div className="small">• Project name: <b>{preview.project_name}</b></div>
            <label className="row small" style={{ gap: 6 }}><input type="checkbox" checked={doImport} onChange={(e) => setDoImport(e.target.checked)} /> Also add its {preview.tasks.length} action item{preview.tasks.length === 1 ? "" : "s"} as shared tasks (title, notes, status)</label>
            {doImport && preview.tasks.length > 0 && (
              <div className="col" style={{ gap: 2, maxHeight: 200, overflow: "auto" }}>
                {preview.tasks.map((t: Row, i: number) => <div key={i} className="mono small dim">[{t.status}] {t.title}{t.notes ? ` — ${t.notes}` : ""}</div>)}
              </div>
            )}
            <div className="dim small">Stays on this PC: {(preview.kept_private as string[]).join(", ")}. You can share its file list + map afterwards from the MAP tab, with its own preview.</div>
          </div>
        </Modal>
      )}
    </div>
  );
}

/* ---------- conflicts ---------- */

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
              {c.kind === "rejected" && <div className="small">The hub refused this edit ({String(c.reason)}). Your text is kept here: <span className="mono">{JSON.stringify(c.mine?.fields ?? c.mine)}</span></div>}
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

/* ---------- one workspace ---------- */

const TABS = ["overview", "tasks", "boards", "media", "map", "files", "chats", "admin"] as const;
type Tab = (typeof TABS)[number];

function WorkspacePage({ w, me, onBack, onErr }: { w: Row; me: string; onBack: () => void; onErr: (e: string) => void }) {
  const board = useAsync(() => api.hubBoard(w.id), [w.id], ["hub-changed"]);
  const [project, setProject] = useState("");
  const [tab, setTab] = useState<Tab>("overview");
  const b = board.data;
  const br = brand(w.name);
  if (board.error) return <ErrBox error={board.error} />;
  if (!b) return <div className="skeleton" />;
  const links: Record<string, string> = b.links ?? {};
  const local = project ? links[project] : undefined;
  const tabs = TABS.filter((t) => t !== "admin" || can(b.role, "admin"));
  const open = (id: string, t: Tab = "tasks") => { setProject(id); setTab(t); };
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="row wrap" style={{ gap: 14, alignItems: "stretch" }}>
        <div className={`acard ${br.img ? "" : "gen"} on`} style={{ ...glowStyle(br.glow), flex: "0 0 200px", cursor: "default" }}>
          {br.img ? <img src={br.img} alt={w.name} /> : <div className="title">{w.name}</div>}
          <div className="shade" />
        </div>
        <div className="col" style={{ flex: 1, gap: 8, justifyContent: "center" }}>
          <div className="row" style={{ gap: 10, alignItems: "baseline" }}>
            <button className="btn sm ghost" onClick={onBack}>← All workspaces</button>
            <h1 style={{ margin: 0 }}>{w.name}</h1><span className="chip">{b.role}</span>
          </div>
          <div className="row wrap" style={{ gap: 6 }}>{(b.members ?? []).map((m: Row) => <span key={m.user_id} className="chip" style={m.user_id === me ? { borderColor: "var(--lime)" } : undefined} title={m.title ? `${m.title} (${m.role})` : m.role}>{m.name} · {m.title ?? m.role}</span>)}</div>
          <div className="dim small">{b.pending ? `${b.pending} change${b.pending === 1 ? "" : "s"} waiting to sync · ` : ""}{b.last_sync ? `synced ${ago(b.last_sync)}` : "not synced yet"}</div>
        </div>
      </div>
      <Conflicts ws={w.id} list={b.conflicts ?? []} reload={board.reload} onErr={onErr} />
      <div className="row wrap" style={{ gap: 6 }}>
        <button className={`btn sm ${!project ? "primary" : ""}`} onClick={() => setProject("")}>All projects</button>
        {(b.projects ?? []).map((p: Row) => <button key={p.id} className={`btn sm ${project === p.id ? "primary" : ""}`} onClick={() => setProject(p.id)}>{p.name}{links[p.id] ? " ·" : ""}</button>)}
      </div>
      <nav className="row wrap" style={{ gap: 4 }}>
        <span className="dim">[</span>
        {tabs.map((t) => <button key={t} className={`btn sm ghost ${tab === t ? "on" : ""}`} style={tab === t ? { color: br.glow, borderColor: br.glow } : undefined} onClick={() => setTab(t)}>{t.toUpperCase()}</button>)}
        <span className="dim">]</span>
      </nav>
      <section className="card"><div className="card-body">
        {tab === "overview" && ((b.projects ?? []).length ? (
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))", gap: 12 }}>
            {(b.projects ?? []).filter((p: Row) => !project || p.id === project).map((p: Row) => <ProjectTile key={p.id} p={p} b={b} local={links[p.id]} glow={br.glow} onOpen={() => open(p.id)} />)}
          </div>
        ) : <Empty>{can(b.role, "admin") ? "No shared projects yet. Open ADMIN to create one or share a project from this PC." : "No shared projects you can see yet."}</Empty>)}
        {tab === "tasks" && <Tasks ws={w.id} b={b} project={project} me={me} onErr={onErr} reload={board.reload} />}
        {tab === "boards" && <Boards ws={w.id} b={b} project={project} me={me} onErr={onErr} reload={board.reload} />}
        {tab === "media" && <Media ws={w.id} b={b} project={project} onErr={onErr} reload={board.reload} />}
        {(tab === "map" || tab === "files") && <Structure key={project + tab} ws={w.id} b={b} project={project} glow={br.glow} mode={tab} local={local} me={me} onErr={onErr} reload={board.reload} />}
        {tab === "chats" && (project ? <LocalChats local={local} /> : <Empty>Pick a project above to see its chats from this PC.</Empty>)}
        {tab === "admin" && <Admin ws={w.id} b={b} me={me} reload={board.reload} onErr={onErr} />}
      </div></section>
    </div>
  );
}

/* ---------- TEAM ---------- */

export default function Team() {
  const status = useAsync(() => api.hubStatus(), [], ["hub-changed"]);
  const host = useAsync(() => api.hubHostStatus(), []);
  const [tsBusy, setTsBusy] = useState(false);
  const [open, setOpen] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [leaving, setLeaving] = useState(false);
  const [guide, setGuide] = useState<boolean | null>(null);
  const [device, setDevice] = useState<Row | null>(null);
  const s = status.data;
  const list: Row[] = s?.workspaces ?? [];
  const owns = list.some((w) => w.role === "owner");
  const people = useAsync(() => (owns ? api.hubAdmin("", "people") : Promise.resolve({ people: [] })), [owns], ["hub-changed"]);
  const others = (people.data?.people ?? []).length;
  const current = list.find((w) => w.id === open);
  useEffect(() => { if (open && !current && s) setOpen(null); }, [open, current, s]);
  return (
    <div className="col" style={{ gap: 14 }}>
      {!current && (
        <div className="page-head">
          <div><h1>Team</h1><div className="sub">Shared workspaces on your own hub · only what you share leaves this PC</div></div>
          {s?.joined && (
            <div className="row" style={{ gap: 8, marginLeft: "auto", alignItems: "center" }}>
              <span className="small"><span className={`dot ${s.online === false ? "busy" : ""}`} /> {s.online === false ? "Hub offline · showing last sync" : s.online ? "Hub online" : "Connecting…"}</span>
              {host.data && !host.data.tailscale && <span className="small" style={{ color: "var(--gold)" }} title="Teammates reach the hub over Tailscale">Tailscale off: only this PC can reach the hub</span>}
              {host.data?.tailscale_state != null && (
                <button className="btn sm" disabled={tsBusy} title="Phones, teammates and the https address reach this PC over Tailscale"
                  onClick={() => { setTsBusy(true); setErr(null); api.tailscaleSet(!host.data?.tailscale).then(() => host.reload(), (e) => setErr(String(e))).finally(() => setTsBusy(false)); }}>
                  {tsBusy ? "Switching…" : host.data.tailscale ? "Turn Tailscale off" : "Turn Tailscale on"}
                </button>
              )}
              <button className="btn sm" onClick={() => api.hubSyncNow()}>Sync now</button>
              <button className="btn sm" onClick={() => api.hubDeviceLink().then(setDevice, (e) => setErr(String(e)))} title="Sign your phone, tablet or another PC in as you">Add another device</button>
              {leaving
                ? <button className="btn sm danger" onClick={() => api.hubLeave().then(() => { setLeaving(false); status.reload(); }, (e) => setErr(String(e)))}>Confirm: leave the hub on this PC</button>
                : <button className="btn sm ghost" onClick={() => setLeaving(true)}>Leave</button>}
            </div>
          )}
        </div>
      )}
      {err && <div onClick={() => setErr(null)} style={{ cursor: "pointer" }}><ErrBox error={err} /></div>}
      {status.error && <ErrBox error={status.error} />}
      {s && !s.joined && <Start host={host.data} onDone={() => { status.reload(); host.reload(); }} />}
      {s?.revoked && <ErrBox error="This PC was signed out of the hub. Your unsent edits were saved in the app's hub\unsent folder. Leave, then join again with a new invite." />}
      {s?.joined && !s.revoked && !current && owns && <ConnectGuide host={host.data} people={others} open={guide ?? (!host.data?.tailscale || others === 0)} setOpen={setGuide} />}
      {s?.joined && !s.revoked && !current && (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))", gap: 14 }}>
          {list.map((w) => <WsCard key={w.id} w={w} onOpen={() => setOpen(w.id)} />)}
          {host.data?.installed && /:\/\/(127\.0\.0\.1|localhost):8770$/.test(String(s.url ?? "")) && <NewWsCard me={s.name ?? ""} onDone={status.reload} onErr={setErr} />}
          {!list.length && !host.data?.installed && <Empty>You're not in any workspace on this hub right now.</Empty>}
        </div>
      )}
      {s?.joined && !s.revoked && !current && owns && <People onErr={setErr} />}
      {s?.joined && !s.revoked && !current && (
        <label className="row small" style={{ gap: 8 }} title="Copies into 66 MEDIA ARCHIVE/<project>/WORKING/<folder>; never overwrites or deletes; images, video, audio and PDFs only">
          <input type="checkbox" checked={!!s.vault_media} onChange={(e) => api.hubSetVaultMedia(e.target.checked).then(status.reload, (er) => setErr(String(er)))} />
          Save team media into my Book of Secrets (66 MEDIA ARCHIVE › project › WORKING › folder)
        </label>
      )}
      {s?.joined && !s.revoked && !current && <JoinMore me={s.name ?? ""} onDone={status.reload} onErr={setErr} />}
      {device && (
        <Modal title="Add another device" onClose={() => setDevice(null)} foot={<button className="btn primary" onClick={() => setDevice(null)}>Done</button>}>
          <LinkBox link={device.link} qr={device.qr_svg} note="Scan with your iPhone or iPad camera, or open the link on another PC (TEAM › paste it). Works once, for 15 minutes. Tailscale must be on there too." />
        </Modal>
      )}
      {current && s && <WorkspacePage key={current.id} w={current} me={s.user_id ?? ""} onBack={() => setOpen(null)} onErr={setErr} />}
    </div>
  );
}
