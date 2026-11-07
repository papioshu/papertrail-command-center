import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, type Row } from "../api";
import { AgentChip, ago, brand, Empty, ErrBox, sessionState, useAsync, useNav } from "../ui";
import { foldEntries, stepHead, toolSummary, type Block, type ChatEntry, type Step } from "../chat/fold";
import { inline, mdBlocks } from "../chat/md";

const UNMAPPED = "Not linked to a project";

/** CHAT: one card per project, then that project's chats, then any one chat as its complete conversation. */
export default function Chat() {
  const { params } = useNav();
  if (params.id) return <Conversation id={String(params.id)} />;
  return params.project != null ? <ProjectChats project={String(params.project)} /> : <ProjectCards />;
}

/** A project's chats as one card. `c` is a chatProjects row; id '' collects chats with no known project. */
export function ChatCard({ c }: { c: Row }) {
  const { nav } = useNav();
  const st = sessionState(c.updated);
  const workspace = String(c.workspace ?? "");
  const n = c.chats ?? 0;
  return (
    <button className={`chat-card ${st.cls}`} style={{ ["--glow" as string]: brand(workspace).glow }} disabled={!n}
      onClick={() => nav("chat", { project: c.id, name: c.name ?? UNMAPPED })}>
      <div className="row" style={{ gap: 8 }}>
        <span className="label ellipsis" style={{ color: "var(--glow)" }}>{workspace || "No project"}</span>
        {st.cls === "live" ? <span className="chip live" style={{ marginLeft: "auto" }}>writing now</span> : n > 0 && <span className="label" style={{ marginLeft: "auto", whiteSpace: "nowrap" }}>Chats →</span>}
      </div>
      <b className="ellipsis" style={{ fontSize: 15 }}>{c.name ?? UNMAPPED}</b>
      <div className="mono small">{n ? `${n} ${n === 1 ? "chat" : "chats"}` : "No chats yet"}{c.claude ? ` · ${c.claude} Claude` : ""}{c.codex ? ` · ${c.codex} Codex` : ""}{c.temp_chats ? ` · ${c.temp_chats} in a temp copy` : ""}</div>
      {n > 0 && <div className="muted small ellipsis">{ago(c.updated)} · {c.last_title || "(untitled)"}</div>}
    </button>
  );
}

const TempCopy = ({ name }: { name?: string | null }) =>
  name ? <span className="chip warn" style={{ whiteSpace: "nowrap" }} title={`This chat ran in the temporary working copy ${name}`}>TEMP COPY · {name}</span> : null;

function ProjectCards() {
  const [q, setQ] = useState("");
  const list = useAsync(() => api.chatProjects(), [], ["projects-updated"]);
  const needle = q.trim().toLowerCase();
  const shown = (list.data ?? [])
    .filter((c) => !needle || [c.name ?? UNMAPPED, c.workspace, c.titles].some((s) => String(s ?? "").toLowerCase().includes(needle)))
    .sort((a, b) => Number(sessionState(b.updated).cls === "live") - Number(sessionState(a.updated).cls === "live"));
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="page-head">
        <div><h1>Chat</h1><div className="sub">One card per project · open a card to see its Claude Code and Codex chats</div></div>
        <div className="right">
          <input className="input" placeholder="Filter chats…" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Filter by project or chat title" />
        </div>
      </div>
      {list.error && <ErrBox error={list.error} />}
      {list.data && !shown.length && <Empty>No chats match.</Empty>}
      <div className="chat-cards">{shown.map((c) => <ChatCard key={c.id} c={c} />)}</div>
    </div>
  );
}

function ProjectChats({ project }: { project: string }) {
  const { nav, params } = useNav();
  const [agent, setAgent] = useState("");
  const [q, setQ] = useState("");
  const list = useAsync(() => api.chatList(project, agent || undefined), [project, agent], ["projects-updated"]);
  const needle = q.trim().toLowerCase();
  const shown = (list.data ?? [])
    .filter((r) => !needle || [r.title, r.last_reply].some((s) => String(s ?? "").toLowerCase().includes(needle)))
    .sort((a, b) => Number(sessionState(b.updated).cls === "live") - Number(sessionState(a.updated).cls === "live"));
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="row" style={{ gap: 8 }}>
        <button className="btn ghost sm" onClick={() => nav("chat")}>← All projects</button>
        {project && <button className="btn ghost sm" onClick={() => nav("projects", { id: project })}>Project page</button>}
      </div>
      <div className="page-head">
        <div><h1>{params.name ?? UNMAPPED}</h1><div className="sub">{list.data ? `${list.data.length} ${list.data.length === 1 ? "chat" : "chats"}` : "Loading…"} · open one to read it in full</div></div>
        <div className="right">
          <input className="input" placeholder="Filter chats…" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Filter chats" />
          <div className="seg">
            {["", "claude", "codex"].map((a) => <button key={a} className={agent === a ? "on" : ""} onClick={() => setAgent(a)}>{a ? a.toUpperCase() : "ALL"}</button>)}
          </div>
        </div>
      </div>
      {list.error && <ErrBox error={list.error} />}
      {list.data && !shown.length && <Empty>No chats match.</Empty>}
      <div className="col chat-list">
        {shown.slice(0, 300).map((r) => {
          const st = sessionState(r.updated);
          return (
            <button key={r.id} className={`chat-row ${st.cls}`} onClick={() => nav("chat", { id: r.id })}>
              <div className="row" style={{ gap: 8 }}>
                <AgentChip agent={r.agent} />
                <TempCopy name={r.temp_copy} />
                <b className="ellipsis">{r.title || "(untitled)"}</b>
                <span className="dim small mono" style={{ marginLeft: "auto", whiteSpace: "nowrap" }}>
                  {st.cls === "live" ? "writing now · " : ""}{ago(r.updated)} · {r.prompts ?? 0} prompts · {r.edits ?? 0} edits
                </span>
              </div>
              {r.last_reply && <div className="muted small ellipsis" style={{ marginTop: 4 }}>{r.last_reply}</div>}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function Conversation({ id }: { id: string }) {
  const { nav } = useNav();
  const [session, setSession] = useState<Row | null>(null);
  const [entries, setEntries] = useState<ChatEntry[]>([]);
  const [cursor, setCursor] = useState<number | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [expandAll, setExpandAll] = useState(false);
  const [showSystem, setShowSystem] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [force, setForce] = useState(false);
  const end = useRef(0);
  const current = useRef(id);
  const busy = useRef(false);
  const box = useRef<HTMLDivElement>(null);
  const keep = useRef<{ mode: "bottom" | "anchor"; h?: number } | null>(null);

  async function load() {
    setErr(null);
    try {
      const r = await api.chatPage(id);
      if (current.current !== id) return; // a newer chat was opened meanwhile
      setSession(r.session);
      setEntries(r.page.entries);
      setCursor(r.page.cursor ?? null);
      end.current = r.page.end;
      keep.current = { mode: "bottom" };
    } catch (e) { setErr(String(e)); }
  }
  useEffect(() => { current.current = id; end.current = 0; setEntries([]); setSession(null); load(); }, [id]);

  // Live tail every 3 s; stick to the bottom only if the reader was already there.
  useEffect(() => {
    const t = setInterval(async () => {
      if (!end.current || busy.current) return;
      busy.current = true;
      try {
        const p = await api.chatTail(id, end.current);
        if (current.current !== id) return;
        if (p.reset) return load();
        end.current = p.end;
        if (!p.entries.length) return;
        const el = box.current;
        if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 80) keep.current = { mode: "bottom" };
        setEntries((xs) => [...xs, ...p.entries]);
      } catch { /* transient: next tick retries */ } finally { busy.current = false; }
    }, 3000);
    return () => clearInterval(t);
  }, [id]);

  useLayoutEffect(() => {
    const el = box.current, k = keep.current;
    if (!el || !k) return;
    el.scrollTop = k.mode === "bottom" ? el.scrollHeight : el.scrollHeight - (k.h ?? 0);
    keep.current = null;
  }, [entries]);

  async function earlier() {
    if (cursor == null || busy.current) return;
    busy.current = true;
    try {
      const r = await api.chatPage(id, cursor);
      if (current.current !== id) return;
      keep.current = { mode: "anchor", h: (box.current?.scrollHeight ?? 0) - (box.current?.scrollTop ?? 0) };
      setEntries((xs) => [...r.page.entries, ...xs]);
      setCursor(r.page.cursor ?? null);
    } catch (e) { setErr(String(e)); } finally { busy.current = false; }
  }
  async function full(off: number) {
    try {
      const fresh = await api.chatEntry(id, off);
      setEntries((xs) => [...xs.filter((x) => x.off < off), ...fresh, ...xs.filter((x) => x.off > off)]);
    } catch (e) { setErr(String(e)); }
  }
  async function terminal() {
    if (!session?.project_id) return;
    setNote(null);
    try {
      const r = await api.launch({ project_id: session.project_id, agent: session.agent, mode: "continue", session_id: id, force });
      if (r.status === "conflict") { setForce(true); setNote(`${r.message} Click again to launch anyway.`); }
      else { setForce(false); setNote(`Opened in a terminal: ${r.command}`); }
    } catch (e) { setErr(String(e)); }
  }
  const resumeCmd = session ? `cd "${session.cwd ?? ""}"; ${session.agent === "codex" ? "codex resume" : "claude --resume"} ${id}` : "";
  const blocks = useMemo(() => foldEntries(entries, showSystem), [entries, showSystem]);
  const toggle = (set: (v: boolean) => void, v: boolean) => {
    const el = box.current;
    if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 80) keep.current = { mode: "bottom" };
    set(v);
  };
  useLayoutEffect(() => {
    const el = box.current, k = keep.current;
    if (el && k?.mode === "bottom") { el.scrollTop = el.scrollHeight; keep.current = null; }
  }, [expandAll, showSystem]);

  return (
    <div className="col chat-view" style={{ gap: 10 }}>
      <div className="row wrap" style={{ gap: 10, alignItems: "center" }}>
        <button className="btn ghost sm" onClick={() => session ? nav("chat", { project: session.card_id ?? "", name: session.card_name ?? UNMAPPED }) : nav("chat")}>← Chats</button>
        {session && <AgentChip agent={session.agent} />}
        <TempCopy name={session?.temp_copy} />
        <b className="ellipsis" style={{ maxWidth: 520 }}>{session?.title || id}</b>
        {session && <span className="dim small mono ellipsis">{session.project ?? UNMAPPED} · {session.cwd ?? "—"} · {session.branch ?? "—"}</span>}
        <div className="row wrap" style={{ gap: 6, marginLeft: "auto" }}>
          <label className="small"><input type="checkbox" checked={expandAll} onChange={(e) => toggle(setExpandAll, e.target.checked)} /> Expand all tool steps</label>
          <label className="small"><input type="checkbox" checked={showSystem} onChange={(e) => toggle(setShowSystem, e.target.checked)} /> Show system entries</label>
          <button className="btn sm desk-only" disabled={!session?.project_id} title={session?.project_id ? "" : "Map this chat to a project first"} onClick={terminal}>Continue in terminal</button>
          <button className="btn sm ghost desk-only" disabled={!session?.cwd} onClick={() => api.openPath(session!.cwd)}>Open folder</button>
          <button className="btn sm ghost desk-only" disabled={!session} onClick={() => navigator.clipboard.writeText(resumeCmd).then(() => setNote("Resume command copied."))}>Copy resume command</button>
        </div>
      </div>
      {note && <div className="dim small mono">{note}</div>}
      {err && <ErrBox error={err} />}
      <div ref={box} className="chat-log">
        {cursor != null && <button className="btn sm ghost" style={{ alignSelf: "center" }} onClick={earlier}>Load earlier</button>}
        {!err && session && !blocks.length && <Empty>Nothing to show yet.</Empty>}
        {blocks.map((b) => <BlockView key={b.kind === "tools" ? `t${b.key}` : `${b.entry.off}:${b.entry.seq}`} b={b} open={expandAll} full={full} />)}
      </div>
    </div>
  );
}

function Imgs({ e, full }: { e: ChatEntry; full: (off: number) => void }) {
  if (!e.images.length) return null;
  return (
    <div className="col" style={{ gap: 6, marginTop: 6 }}>
      {e.images.map((im, i) => im.data
        ? <img key={i} className="chat-img" src={`data:${im.media_type};base64,${im.data}`} alt="image from the chat" />
        : <button key={i} className="btn sm ghost" onClick={() => full(e.off)}>Image too large to preview · Load full</button>)}
    </div>
  );
}

const More = ({ e, full }: { e: ChatEntry; full: (off: number) => void }) =>
  e.truncated ? <button className="btn sm ghost" onClick={() => full(e.off)}>Shortened · Load full</button> : null;

function BlockView({ b, open, full }: { b: Block; open: boolean; full: (off: number) => void }) {
  if (b.kind === "tools")
    return (
      <details className="chat-block chat-tools" open={open || undefined}>
        <summary className="mono small">{toolSummary(b.counts)}</summary>
        {b.steps.map((s) => <StepView key={`${s.use.off}:${s.use.seq}`} s={s} full={full} />)}
      </details>
    );
  const e = b.entry;
  if (b.kind === "you") return <div className="chat-block chat-you">{e.text}<Imgs e={e} full={full} /><More e={e} full={full} /></div>;
  if (b.kind === "agent") return <div className="chat-block chat-agent"><Markdown text={e.text} /><More e={e} full={full} /></div>;
  if (b.kind === "reasoning") return <details className="chat-block chat-reason"><summary className="small dim">Reasoning</summary><div className="chat-pre">{e.text}</div></details>;
  return <div className="chat-block chat-system">{e.text}<Imgs e={e} full={full} /></div>;
}

function StepView({ s, full }: { s: Step; full: (off: number) => void }) {
  const u = s.use, r = s.result ?? (u.kind === "tool_result" ? u : undefined);
  const inp = u.kind === "tool_use" ? u.input ?? {} : null;
  const { head, rest } = stepHead(inp);
  const { old_string, new_string, ...other } = rest && typeof rest === "object" ? rest : ({} as any);
  let extra = typeof rest === "object" && rest ? (old_string != null ? other : rest) : rest;
  // A lone text field (Codex scripts) reads better as plain text than as escaped JSON.
  if (extra && typeof extra === "object") { const v = Object.values(extra); if (v.length === 1 && typeof v[0] === "string") extra = v[0]; }
  return (
    <div className="chat-step">
      <div className="mono small"><b>{u.kind === "tool_use" ? u.name : "result"}</b>{head != null && <span className="dim"> · {head}</span>}</div>
      {old_string != null && <><div className="label">−</div><pre className="chat-pre del">{old_string}</pre><div className="label">+</div><pre className="chat-pre add">{new_string}</pre></>}
      {inp && extra != null && (typeof extra !== "object" || Object.keys(extra).length > 0) && <pre className="chat-pre dim">{typeof extra === "string" ? extra : JSON.stringify(extra, null, 2)}</pre>}
      <More e={u} full={full} />
      {r && (r.text || r.images.length) ? <><pre className={`chat-pre ${r.is_error ? "err" : ""}`}>{r.text}</pre><Imgs e={r} full={full} /><More e={r} full={full} /></> : null}
    </div>
  );
}

function Inl({ s }: { s: string }) {
  return <>{inline(s).map((p, i) => p.t === "code" ? <code key={i}>{p.s}</code> : p.t === "bold" ? <b key={i}>{p.s}</b> : <Fragment key={i}>{p.s}</Fragment>)}</>;
}

function Markdown({ text }: { text: string }) {
  const out: ReactNode[] = [];
  mdBlocks(text).forEach((b, i) => {
    if (b.t === "code") out.push(<pre key={i} className="chat-pre">{b.body}</pre>);
    else if (b.t === "h") out.push(<div key={i} className={`chat-h h${b.level}`}><Inl s={b.text} /></div>);
    else if (b.t === "li") out.push(<div key={i} className="chat-li">{b.ordered ? "· " : "• "}<Inl s={b.text} /></div>);
    else out.push(<p key={i} className="chat-p"><Inl s={b.text} /></p>);
  });
  return <>{out}</>;
}
