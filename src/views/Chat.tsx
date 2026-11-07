import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, type Row } from "../api";
import { AgentChip, ago, Empty, ErrBox, sessionState, useAsync, useNav } from "../ui";
import { foldEntries, toolSummary, type Block, type ChatEntry, type Step } from "../chat/fold";
import { inline, mdBlocks } from "../chat/md";

/** CHAT: every Claude Code / Codex chat, and any one of them as its complete conversation. */
export default function Chat() {
  const { params } = useNav();
  return params.id ? <Conversation id={String(params.id)} /> : <ChatList />;
}

function ChatList() {
  const { nav } = useNav();
  const [agent, setAgent] = useState("");
  const [project, setProject] = useState("");
  const [q, setQ] = useState("");
  const list = useAsync(() => api.chatList(agent || undefined), [agent], ["projects-updated"]);
  const all = list.data ?? [];
  const projects = useMemo(() => [...new Set(all.map((r) => r.project).filter(Boolean))].sort() as string[], [all]);
  const needle = q.trim().toLowerCase();
  const shown = all
    .filter((r) => !project || r.project === project)
    .filter((r) => !needle || [r.title, r.project, r.last_reply].some((s) => String(s ?? "").toLowerCase().includes(needle)))
    .sort((a, b) => Number(sessionState(b.updated).cls === "live") - Number(sessionState(a.updated).cls === "live"));
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="page-head">
        <div><h1>Chat</h1><div className="sub">Every Claude Code and Codex chat · open one to read it in full</div></div>
        <div className="right">
          <input className="input" placeholder="Filter chats…" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Filter chats" />
          <select className="input" value={project} onChange={(e) => setProject(e.target.value)} aria-label="Project">
            <option value="">All projects</option>
            {projects.map((p) => <option key={p} value={p}>{p}</option>)}
          </select>
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
                <span className="chip">{r.project ?? "unmapped"}</span>
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
  const box = useRef<HTMLDivElement>(null);
  const keep = useRef<{ mode: "bottom" | "anchor"; h?: number } | null>(null);

  async function load() {
    setErr(null);
    try {
      const r = await api.chatPage(id);
      setSession(r.session);
      setEntries(r.page.entries);
      setCursor(r.page.cursor ?? null);
      end.current = r.page.end;
      keep.current = { mode: "bottom" };
    } catch (e) { setErr(String(e)); }
  }
  useEffect(() => { setEntries([]); setSession(null); load(); }, [id]);

  // Live tail every 3 s; stick to the bottom only if the reader was already there.
  useEffect(() => {
    const t = setInterval(async () => {
      if (!end.current) return;
      try {
        const p = await api.chatTail(id, end.current);
        if (p.reset) return load();
        end.current = p.end;
        if (!p.entries.length) return;
        const el = box.current;
        if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 80) keep.current = { mode: "bottom" };
        setEntries((xs) => [...xs, ...p.entries]);
      } catch { /* transient: next tick retries */ }
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
    if (cursor == null) return;
    try {
      const r = await api.chatPage(id, cursor);
      keep.current = { mode: "anchor", h: (box.current?.scrollHeight ?? 0) - (box.current?.scrollTop ?? 0) };
      setEntries((xs) => [...r.page.entries, ...xs]);
      setCursor(r.page.cursor ?? null);
    } catch (e) { setErr(String(e)); }
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

  return (
    <div className="col" style={{ gap: 10, height: "calc(100vh - 190px)" }}>
      <div className="row wrap" style={{ gap: 10, alignItems: "center" }}>
        <button className="btn ghost sm" onClick={() => nav("chat")}>← Chats</button>
        {session && <AgentChip agent={session.agent} />}
        <b className="ellipsis" style={{ maxWidth: 520 }}>{session?.title || id}</b>
        {session && <span className="dim small mono ellipsis">{session.project ?? "unmapped"} · {session.cwd ?? "—"} · {session.branch ?? "—"}</span>}
        <div className="row" style={{ gap: 6, marginLeft: "auto" }}>
          <label className="small"><input type="checkbox" checked={expandAll} onChange={(e) => setExpandAll(e.target.checked)} /> Expand all tool steps</label>
          <label className="small"><input type="checkbox" checked={showSystem} onChange={(e) => setShowSystem(e.target.checked)} /> Show system entries</label>
          <button className="btn sm" disabled={!session?.project_id} title={session?.project_id ? "" : "Map this chat to a project first"} onClick={terminal}>Continue in terminal</button>
          <button className="btn sm ghost" disabled={!session?.cwd} onClick={() => api.openPath(session!.cwd)}>Open folder</button>
          <button className="btn sm ghost" disabled={!session} onClick={() => navigator.clipboard.writeText(resumeCmd).then(() => setNote("Resume command copied."))}>Copy resume command</button>
        </div>
      </div>
      {note && <div className="dim small mono">{note}</div>}
      {err && <ErrBox error={err} />}
      <div ref={box} className="chat-log">
        {cursor != null && <button className="btn sm ghost" style={{ alignSelf: "center" }} onClick={earlier}>Load earlier</button>}
        {!err && session && !blocks.length && <Empty>Nothing to show yet.</Empty>}
        {blocks.map((b) => <BlockView key={b.kind === "tools" ? `t${b.key}` : `${b.entry.off}:${b.entry.kind}:${b.entry.text.length}`} b={b} open={expandAll} full={full} />)}
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
        {b.steps.map((s) => <StepView key={s.use.off + ":" + (s.use.tool_id ?? "")} s={s} full={full} />)}
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
  const head = inp ? inp.command ?? inp.cmd ?? inp.file_path ?? inp.notebook_path ?? inp.pattern ?? inp.path ?? null : null;
  return (
    <div className="chat-step">
      <div className="mono small"><b>{u.kind === "tool_use" ? u.name : "result"}</b>{head != null && <span className="dim"> · {typeof head === "string" ? head : JSON.stringify(head)}</span>}</div>
      {inp && inp.old_string != null && <><div className="label">−</div><pre className="chat-pre del">{inp.old_string}</pre><div className="label">+</div><pre className="chat-pre add">{inp.new_string}</pre></>}
      {inp && inp.old_string == null && (head == null || Object.keys(inp).length > 1) && <pre className="chat-pre">{JSON.stringify(inp, null, 2)}</pre>}
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
