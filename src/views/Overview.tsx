import { lazy, Suspense, useMemo, useState, type CSSProperties, type ReactNode } from "react";
import { api, type Row } from "../api";
import { ago, AgentChip, Barcode, brand, day, Empty, ErrBox, orderDiscoveries, ProjectActions, RadarOrderControl, sessionState, short, Spark, StatusBar, useAsync, useDragScroll, useNav, type RadarOrder } from "../ui";

export const counts = (p: Row) => ({ done: p.done_tasks, active: p.active_tasks, pending: p.pending_tasks, blocked: p.blocked_tasks, deferred: p.deferred_tasks });
export const sumCounts = (ps: Row[]) => ps.reduce((a: Record<string, number>, p) => { for (const [k, v] of Object.entries(counts(p))) a[k] = (a[k] || 0) + (v as number); return a; }, {});

const REFRESH = ["projects-updated"];
// The sphere is its own chunk: the dashboard paints first, the GPU scene arrives after.
const Sphere = lazy(() => import("../sphere/Sphere"));

/** Front-page radar: top matched tools / repos / releases, with the evidence for each match. */
function Radar({ names }: { names: (id: string) => string }) {
  const { nav } = useNav();
  const d = useAsync(() => api.discoveries(), [], ["discovery-updated"]);
  const ref = useDragScroll<HTMLDivElement>();
  const [busy, setBusy] = useState(false);
  const [mode, setMode] = useState<RadarOrder>("mix");
  const [seed, setSeed] = useState(() => Date.now());
  const items: Row[] = useMemo(() => orderDiscoveries((d.data?.items ?? []).filter((x: Row) => x.status !== "dismissed"), mode, seed).slice(0, 24), [d.data, mode, seed]);
  const set = (id: string, st: string) => api.setDiscovery(id, st).then(d.reload);
  return (
    <>
      <div className="sect">
        <h2>Radar · comparable tools</h2><div className="rule" />
        <span className="aside">
          {d.data?.last ? `refreshed ${ago(d.data.last)}` : "never refreshed"}
          <RadarOrderControl mode={mode} setMode={setMode} reshuffle={() => setSeed(Date.now())} />
          <button className="btn sm" disabled={busy} onClick={() => { setBusy(true); api.discoveryRefresh().finally(() => setTimeout(() => setBusy(false), 60000)); }}>{busy ? "Fetching…" : "Refresh"}</button>
          <button className="btn sm" onClick={() => nav("discovery")}>All</button>
        </span>
      </div>
      <div className="deck" ref={ref} style={{ cursor: "grab", paddingTop: 4 }}>
        {items.map((x) => {
          const ps: string[] = JSON.parse(x.projects || "[]");
          return (
            <div key={x.id} className="card" style={{ flex: "0 0 300px", display: "flex", flexDirection: "column" }}>
              <div className="card-head">
                <span className={`chip ${x.source === "npm" ? "warn" : ""}`}>{x.source === "npm" ? "release" : "repo"}</span>
                <span className="ellipsis" style={{ fontWeight: 600 }} title={x.name}>{x.name}</span>
                <div className="right">{x.stars != null && <span className="mono small dim">{x.stars}★</span>}</div>
              </div>
              <div className="card-body col" style={{ gap: 6, flex: 1 }}>
                <div className="small clamp2">{x.description || <span className="dim">No description</span>}</div>
                <div className="row wrap">{ps.slice(0, 3).map((p) => <span key={p} className="chip active">{names(p)}</span>)}<span className={`chip ${String(x.pricing).startsWith("Free") ? "ok" : "warn"}`}>{String(x.pricing).replace("Free · open source", "OSS")}</span></div>
                <div className="dim small clamp2" title={x.reason}>{x.reason}</div>
                <div className="row" style={{ marginTop: "auto" }}>
                  <button className="btn sm" onClick={() => api.openUrl(x.url)}>Open</button>
                  {x.status !== "saved" ? <button className="btn sm" onClick={() => set(x.id, "saved")}>Save</button> : <span className="chip ok">saved</span>}
                  <button className="btn sm ghost" onClick={() => set(x.id, "dismissed")}>Dismiss</button>
                </div>
              </div>
            </div>
          );
        })}
        {!items.length && <Empty>{d.loading ? "Loading…" : "Nothing fetched yet. Refresh pulls new repos and releases matched to your projects' dependencies."}</Empty>}
      </div>
    </>
  );
}

/** Right-hand panel: the hovered or selected project, with its status and real actions. */
function Focus({ p, spark }: { p: Row | undefined; spark: number[] }) {
  const { nav } = useNav();
  if (!p) return <div className="empty">Hover a node to inspect a project</div>;
  const b = brand(p.workspace);
  const claude = p.last_claude ? JSON.parse(p.last_claude) : null;
  const codex = p.last_codex ? JSON.parse(p.last_codex) : null;
  return (
    <section className="card glow" style={glowStyle(b.glow)}>
      <div className="card-head"><span className="label" style={{ color: b.glow }}>{p.workspace}</span><div className="right"><Barcode value={p.id} width={54} height={14} /></div></div>
      <div className="card-body col" style={{ gap: 12 }}>
        <div className="display" style={{ fontSize: 20, letterSpacing: ".12em", cursor: "pointer" }} onClick={() => nav("projects", { id: p.id })}>{p.name}</div>
        <div>
          <div className="metaline"><span>Last activity</span><b>{ago(p.last_activity)}</b></div>
          <div className="metaline"><span>Branch</span><b className="ellipsis" style={{ maxWidth: 150 }}>{p.branch ?? "—"}{p.dirty ? ` · ${p.dirty} dirty` : ""}</b></div>
          <div className="metaline"><span>Claude</span><b>{claude ? ago(claude.updated) : "—"}</b></div>
          <div className="metaline"><span>Codex</span><b>{codex ? ago(codex.updated) : "—"}</b></div>
        </div>
        <div><div className="label" style={{ fontSize: 8.5, marginBottom: 5 }}>Tasks by status</div><StatusBar counts={counts(p)} legend /></div>
        <div><div className="label" style={{ fontSize: 8.5, marginBottom: 3 }}>Recorded activity · 14 days</div><Spark values={spark} width={270} height={30} color={b.glow} /></div>
        {p.next_action && <div className="small muted clamp3">{String(p.next_action).replace(/^\*\*Next logical (action|task):\*\*\s*/, "")}</div>}
        <ProjectActions p={p} compact />
      </div>
    </section>
  );
}
const glowStyle = (glow: string) => ({ ["--glow" as string]: glow }) as CSSProperties;

/** One access card per workspace: PapertrailOS profile art where it exists, a drawn card otherwise. */
function AccessCard({ ws, projects, on, lit, onClick, onHover }: { ws: string; projects: Row[]; on: boolean; lit: boolean; onClick: () => void; onHover: (ws: string | null) => void }) {
  const b = brand(ws);
  const last = projects.map((p) => p.last_activity).filter(Boolean).sort().pop();
  const blocked = projects.reduce((a, p) => a + p.blocked_tasks, 0);
  // Radar blips: one per project; distance from centre = days since last activity (closer = more recent).
  const blips = projects.map((p, i) => {
    const days = p.last_activity ? (Date.now() - Date.parse(p.last_activity.length === 10 ? p.last_activity + "T12:00:00" : p.last_activity)) / 864e5 : 90;
    const r = 8 + Math.min(1, Math.sqrt(Math.max(0, days) / 60)) * 42;
    const a = (i / Math.max(1, projects.length)) * Math.PI * 2 - Math.PI / 3;
    return { x: 60 + r * Math.cos(a), y: 60 + r * Math.sin(a), name: p.name };
  });
  return (
    <button className={`acard ${b.img ? "" : "gen"} ${on ? "on" : ""} ${lit ? "lit" : ""}`} style={glowStyle(b.glow)} onClick={onClick} aria-pressed={on} title={ws}
      onMouseEnter={() => onHover(ws)} onMouseLeave={() => onHover(null)} onFocus={() => onHover(ws)} onBlur={() => onHover(null)}>
      {b.img ? <img src={b.img} alt={ws} /> : (<><div className="title">{ws}</div>
        <svg className="radar" viewBox="0 0 120 120" aria-hidden>
          {[50, 34, 18].map((r) => <circle key={r} cx={60} cy={60} r={r} fill="none" stroke="currentColor" strokeOpacity={0.35} />)}
          <path d="M60 6V114M6 60H114" stroke="currentColor" strokeOpacity={0.18} />
          <path d="M60 60 L60 10 A50 50 0 0 1 103 35 Z" fill="currentColor" fillOpacity={0.12} />
          {blips.map((b, i) => <circle key={i} cx={b.x} cy={b.y} r={3} fill="currentColor"><title>{b.name}</title></circle>)}
        </svg></>)}
      {blocked > 0 && <span className="chip warn flag">{blocked} blocked</span>}
      <div className="shade" />
      <div style={{ position: "absolute", left: 14, right: 14, bottom: 46 }}><StatusBar counts={sumCounts(projects)} height={4} /></div>
      <div className="meta">
        <span className="k">{projects.length} project{projects.length > 1 ? "s" : ""}</span>
        <span className="v">{last ? ago(last) : "no activity"}</span>
      </div>
      <span className="arrow">→</span>
    </button>
  );
}

export function Ticket({ p, spark }: { p: Row; spark: number[] }) {
  const { nav } = useNav();
  const claude = p.last_claude ? JSON.parse(p.last_claude) : null;
  const codex = p.last_codex ? JSON.parse(p.last_codex) : null;
  return (
    <div className="ticket" style={glowStyle(brand(p.workspace).glow)}>
      <div className="body">
        <div className="row">
          <span className="label" style={{ fontSize: 9 }}>{p.source === "registry" ? "Book of Secrets" : p.source}</span>
          {p.dirty > 0 && <span className="chip warn" title="uncommitted entries (git status)">{p.dirty} dirty</span>}
          {p.ahead > 0 && <span className="chip warn">{p.ahead} unpushed</span>}
          {p.blocked_tasks > 0 && <span className="chip blocked">{p.blocked_tasks} blocked</span>}
        </div>
        <div className="title ellipsis" onClick={() => nav("projects", { id: p.id })}>{p.name}</div>
        <div className="meta">
          <div><div className="k">Last activity</div><div className="v">{ago(p.last_activity)}</div></div>
          <div><div className="k">Branch</div><div className="v ellipsis">{p.branch ?? "—"}</div></div>
          <div><div className="k">Open · Done</div><div className="v">{p.active_tasks} · {p.done_tasks}</div></div>
          <div><div className="k">Claude</div><div className="v ellipsis" title={claude?.title}>{claude ? ago(claude.updated) : "—"}</div></div>
          <div><div className="k">Codex</div><div className="v ellipsis" title={codex?.title}>{codex ? ago(codex.updated) : "—"}</div></div>
          <div><div className="k">Head</div><div className="v">{short(p.head, 7)}</div></div>
        </div>
        <div className="row" style={{ alignItems: "flex-end", gap: 14 }}>
          <div style={{ flex: 1, minWidth: 0 }}><div className="label" style={{ fontSize: 8.5, marginBottom: 4 }}>Tasks by status</div><StatusBar counts={counts(p)} legend /></div>
          <div title="recorded events per day, last 14 days (commits, log entries, decisions, agent sessions)"><div className="label" style={{ fontSize: 8.5, marginBottom: 2 }}>14 days</div><Spark values={spark} width={112} height={24} color={brand(p.workspace).glow} /></div>
        </div>
        <ProjectActions p={p} compact />
      </div>
      <div className="stub">
        <span className="code">{p.name}</span>
        <Barcode value={p.id} width={52} height={28} />
      </div>
    </div>
  );
}

type Line = { gate: string; text: ReactNode; warn?: boolean; go?: () => void };

/** The startup briefing as a bone-paper pass. Deterministic: every line is a count of indexed records. */
function DailyPass({ b }: { b: Row }) {
  const { nav } = useNav();
  const [all, setAll] = useState(false);
  const lines: Line[] = [];
  for (const p of b.projects as Row[]) {
    const parts: string[] = [];
    if (p.commits) parts.push(`${p.commits} commit${p.commits > 1 ? "s" : ""}${p.claude_commits ? ` (${p.claude_commits} with Claude)` : ""}${p.codex_commits ? ` (${p.codex_commits} with Codex)` : ""}`);
    if (p.logs) parts.push(`${p.logs} log entr${p.logs > 1 ? "ies" : "y"}`);
    if (p.decisions) parts.push(`${p.decisions} decision${p.decisions > 1 ? "s" : ""}`);
    lines.push({ gate: p.name, text: <>{parts.join(" · ")}{p.workstreams ? <>, mostly <b>{p.workstreams}</b></> : null}{p.last_decision ? <>. Latest: “{String(p.last_decision).slice(0, 110)}”</> : null}</>,
      go: () => nav("timeline", { project: p.id, from: String(b.since).slice(0, 10) }) });
  }
  for (const c of b.completed as Row[]) lines.push({ gate: c.project_id, text: <>{c.n} task{c.n > 1 ? "s" : ""} checked done in TASKS.md since last start</>, go: () => nav("projects", { id: c.project_id }) });
  for (const x of (b.unpushed as Row[]).filter((x) => x.ahead > 0)) lines.push({ gate: x.name, warn: true, text: <>{x.ahead} commit{x.ahead > 1 ? "s" : ""} on {x.branch} not pushed</>, go: () => nav("projects", { id: x.id }) });
  for (const x of b.stale as Row[]) lines.push({ gate: x.name, warn: true, text: <>{x.open} open NOW task{x.open > 1 ? "s" : ""}, quiet since {String(x.last_activity).slice(0, 10)}. Worth revisiting.</>, go: () => nav("projects", { id: x.id }) });
  if (b.new_discoveries) lines.push({ gate: "Discovery", text: <>{b.new_discoveries} new tools matched to your projects</>, go: () => nav("discovery") });
  const blocked = (b.blocked as Row[]).reduce((a, x) => a + x.n, 0);
  const shown = all ? lines : lines.slice(0, 6);
  const serial = `PT-${String(b.since).slice(5, 10).replace("-", "")}-${lines.length.toString().padStart(2, "0")}`;
  return (
    <div className="pass">
      <div className="main-part">
        <div className="row" style={{ justifyContent: "space-between" }}>
          <span className="plabel">Papertrail OS · Daily pass</span>
          <span className="plabel">{serial}</span>
        </div>
        <div className="ptitle" style={{ margin: "6px 0 10px" }}>Since {String(b.since).slice(0, 10)}</div>
        {shown.map((l, i) => (
          <div key={i} className="line" onClick={l.go}>
            <span className={`gate ${l.warn ? "warn" : ""}`} title={l.gate}>{l.gate.match(/\((.+)\)/)?.[1] ?? l.gate}</span>
            <span className="txt">{l.text}</span>
          </div>
        ))}
        {!lines.length && <div className="txt" style={{ padding: "8px 0" }}>No recorded changes since then.</div>}
        <div className="row" style={{ justifyContent: "space-between", marginTop: 10 }}>
          <span className="plabel">From git, continuity files and session metadata · no generated text</span>
          {lines.length > 6 && <button className="btn sm" style={{ color: "#2a2438", borderColor: "#9c917f", background: "transparent" }} onClick={() => setAll(!all)}>{all ? "Fewer" : `All ${lines.length}`}</button>}
        </div>
      </div>
      <div className="stub">
        <div style={{ textAlign: "center" }}><div className="plabel">Lines</div><div className="big">{lines.length}</div></div>
        <div style={{ textAlign: "center", cursor: "pointer" }} onClick={() => nav("projects")}><div className="plabel">Blocked</div><div className="big" style={{ color: blocked ? "#8a6a10" : undefined }}>{blocked}</div></div>
        <Barcode value={serial} width={84} height={34} />
      </div>
    </div>
  );
}

export default function Overview() {
  const projects = useAsync(() => api.projects(), [], REFRESH);
  const brief = useAsync(() => api.briefing(), [], REFRESH);
  const sparkRows = useAsync(() => api.sparks(14), [], REFRESH);
  const deckRef = useDragScroll<HTMLDivElement>();
  const sparks = useMemo(() => {
    const days: string[] = [];
    for (let i = 13; i >= 0; i--) { const d = new Date(); d.setDate(d.getDate() - i); days.push(day(d)); }
    const m = new Map<string, number[]>();
    for (const r of sparkRows.data ?? []) {
      const arr = m.get(r.project_id) ?? days.map(() => 0);
      const i = days.indexOf(r.day);
      if (i >= 0) arr[i] += r.n;
      m.set(r.project_id, arr);
    }
    return (id: string) => m.get(id) ?? days.map(() => 0);
  }, [sparkRows.data]);
  const [unsorted, setUnsorted] = useState(false);
  const [focusId, setFocusId] = useState<string | null>(null);
  const [cardWs, setCardWs] = useState<string | null>(null); // workspace card under the pointer
  const [mapWs, setMapWs] = useState<string | null>(null); // workspace of the node under the pointer
  const graph = useAsync(() => api.graph(), [], REFRESH);
  const { nav } = useNav();
  const ps = projects.data ?? [];
  const b = brief.data;

  const workspaces = useMemo(() => {
    const m = new Map<string, Row[]>();
    for (const p of ps) if (unsorted || p.workspace !== "UNSORTED") m.set(p.workspace, [...(m.get(p.workspace) ?? []), p]);
    const last = (list: Row[]) => list.map((p) => p.last_activity ?? "").sort().pop() ?? "";
    return [...m.entries()].sort((a, c) => last(c[1]).localeCompare(last(a[1])));
  }, [ps, unsorted]);
  const ws: string | null = null; // cards open the workspace page now
  const sessions: Row[] = b?.sessions ?? [];
  const live = sessions.filter((s) => sessionState(s.updated).cls).length;
  const now = new Date();
  const hour = now.getHours();

  if (projects.error) return <ErrBox error={projects.error} />;
  return (
    <div>
      <div className="home">
        <div className="side">
          <div>
            <div className="label">{now.toLocaleDateString(undefined, { weekday: "long", day: "2-digit", month: "short" })}</div>
            <div className="display" style={{ fontSize: 26, fontWeight: 300, letterSpacing: ".2em", marginTop: 6 }}>{hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening"}</div>
          </div>
          <div className="board" style={{ flexDirection: "column" }}>
            <div style={{ borderRight: 0, borderBottom: "1px solid var(--line)" }}><div className="v">{ps.length}</div><div className="k">Projects on file</div></div>
            <div style={{ borderRight: 0, borderBottom: "1px solid var(--line)" }}><div className="v">{ps.reduce((a, p) => a + p.active_tasks, 0)}</div><div className="k">Open now</div></div>
            <div style={{ borderRight: 0, borderBottom: "1px solid var(--line)", cursor: "pointer" }} onClick={() => nav("projects")}><div className="v" style={{ color: "var(--gold)" }}>{ps.reduce((a, p) => a + p.blocked_tasks, 0)}</div><div className="k">Blocked</div></div>
            <div style={{ cursor: "pointer" }} onClick={() => nav("agents")}><div className="v" style={{ color: live ? "var(--violet-glow)" : undefined }}>{live}</div><div className="k">Agents active</div></div>
          </div>
          <div className="col" style={{ gap: 2 }}>
            <div className="label" style={{ marginBottom: 4 }}>Signal · since last start</div>
            {(b?.projects ?? []).slice(0, 4).map((x: Row) => (
              <div key={x.id} className="metaline" style={{ cursor: "pointer" }} onMouseEnter={() => setFocusId(x.id)} onClick={() => nav("projects", { id: x.id })}>
                <span className="ellipsis" style={{ maxWidth: 170 }}>{x.name.match(/\((.+)\)/)?.[1] ?? x.name}</span><b>{x.commits ? `${x.commits} commits` : `${x.logs + x.decisions} entries`}</b>
              </div>
            ))}
          </div>
          <div className="row" style={{ marginTop: "auto", gap: 10, color: "var(--dim)" }}><Barcode value={String(b?.since ?? "pt")} width={120} height={16} /><span className="label" style={{ fontSize: 8.5 }}>PT-SPHERE · BOOK OF SECRETS</span></div>
        </div>
        <Suspense fallback={<div className="sphere" />}>
          {ps.length > 0 && <Sphere projects={ps.filter((p) => unsorted || p.workspace !== "UNSORTED")} links={graph.data?.links ?? []} selected={focusId} highlightWorkspace={cardWs}
            onHover={(id) => { if (id) setFocusId(id); setMapWs(id ? ps.find((p) => p.id === id)?.workspace ?? null : null); }} onOpen={(id) => nav("projects", { id })} />}
        </Suspense>
        <div className="side">
          <Focus p={ps.find((p) => p.id === focusId) ?? ps[0]} spark={sparks(focusId ?? ps[0]?.id ?? "")} />
          <div className="dim small mono" style={{ fontSize: 10, letterSpacing: ".08em" }}>Each node is a project; brightness and pulse follow its last recorded activity. Lines are explicit cross-references in Book of Secrets files. Drag to turn · wheel to zoom · click to open.</div>
        </div>
      </div>

      <div className="sect" style={{ marginTop: 18 }}>
        <h2>Workspaces</h2><div className="rule" />
        <label className="aside"><input type="checkbox" checked={unsorted} onChange={(e) => setUnsorted(e.target.checked)} /> include unsorted repos</label>
      </div>
      <div className="deck" ref={deckRef} style={{ cursor: "grab" }}>
        {workspaces.map(([w, list]) => <AccessCard key={w} ws={w} projects={list} on={w === ws} lit={w === mapWs || w === cardWs} onClick={() => nav("workspace", { ws: w })} onHover={setCardWs} />)}
        {!workspaces.length && !projects.loading && <Empty>No projects yet. Set the vault and project roots in Settings.</Empty>}
      </div>

      <Radar names={(id) => { const n = ps.find((p) => p.id === id)?.name ?? id; return n.match(/\((.+)\)/)?.[1] ?? n; }} />


      <div className="sect"><h2>Briefing</h2><div className="rule" /><span className="aside">Since your last start</span></div>
      <div className="split">
        {b ? <DailyPass b={b} /> : brief.error ? <ErrBox error={brief.error} /> : <div className="skeleton" />}
        <section className="card">
          <div className="card-head"><span className="label">Agent radar</span><div className="right"><button className="btn sm" onClick={() => nav("agents")}>All sessions</button></div></div>
          <div className="card-body list" style={{ paddingTop: 4, paddingBottom: 4 }}>
            {sessions.slice(0, 6).map((s, i) => {
              const st = sessionState(s.updated);
              return (
                <div key={i} className="li click" onClick={() => s.project_id && nav("projects", { id: s.project_id })}>
                  <AgentChip agent={s.agent} />
                  <div style={{ minWidth: 0 }}>
                    <div className="ellipsis">{s.title || "(untitled)"}</div>
                    <div className="dim small">{s.project ?? "unmapped"}{s.agent === "claude" ? ` · ${s.prompts} prompts · ${s.edits} edits` : ""}</div>
                  </div>
                  <div className="col" style={{ alignItems: "flex-end", gap: 3 }}>
                    {st.cls && <span className={`chip ${st.cls}`}>{st.label}</span>}
                    <span className="dim small mono">{ago(s.updated)}</span>
                  </div>
                </div>
              );
            })}
            {b && !sessions.length && <Empty>No agent sessions since then</Empty>}
          </div>
        </section>
      </div>
    </div>
  );
}
