import { useMemo, useState } from "react";
import FolderMap from "./FolderMap";
import { ActionBoard } from "./Actions";
import { api, type Row } from "../api";
import {
  ago, AgentChip, Barcode, brand, Card, StatusBar, DataTable, DayChart, Empty, ErrBox, Modal, ProjectActions, RangePicker, rangeFor, sessionState, short, useAsync, useNav, type Col, type Range,
} from "../ui";

const STATUS_ORDER = ["done", "active", "pending", "blocked", "deferred"];
const STATUS_LABEL: Record<string, string> = { done: "Completed", active: "Active (NOW)", pending: "Pending", blocked: "Blocked", deferred: "Deferred" };
const STATUS_COLOR: Record<string, string> = { done: "var(--lime)", active: "var(--violet)", pending: "var(--violet-soft)", blocked: "var(--gold)", deferred: "var(--dim)" };

function ProjectList() {
  const { nav } = useNav();
  const [hidden, setHidden] = useState(false);
  const list = useAsync(() => api.projects(hidden), [hidden], ["projects-updated"]);
  const [edit, setEdit] = useState<Row | null>(null);
  const cols: Col[] = [
    { key: "name", label: "Project", width: "22%" },
    { key: "workspace", label: "Workspace", width: "14%" },
    { key: "last_activity", label: "Last activity", width: "11%", render: (r) => ago(r.last_activity) },
    { key: "branch", label: "Branch", width: "13%" },
    { key: "dirty", label: "Dirty", width: "6%" },
    { key: "active_tasks", label: "Open", width: "6%" },
    { key: "blocked_tasks", label: "Blocked", width: "7%" },
    { key: "source", label: "Source", width: "8%", render: (r) => <span className="chip" title={r.source === "registry" ? "MASTER_PASSDOWN registry" : r.source === "scan" ? "found under a project root" : "registered manually"}>{r.source}</span> },
    { key: "repo_path", label: "Directory", width: "25%", render: (r) => <span className="mono dim">{r.repo_path || r.vault_dir}</span> },
  ];
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="page-head">
        <div><h1>Projects</h1><div className="sub">Registry, manual registrations and repos under your project roots</div></div>
        <div className="right">
          <label className="row small muted"><input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} /> include hidden</label>
        </div>
      </div>
      {list.error && <ErrBox error={list.error} />}
      <Card title={`${list.data?.length ?? 0} projects`} body={false} right={<span className="dim small">Click a row to open · right column edits workspace / hides</span>}>
        <DataTable rows={list.data ?? []} cols={[...cols, { key: "_e", label: "", width: "6%", render: (r) => <button className="btn sm" onClick={(e) => { e.stopPropagation(); setEdit(r); }}>Edit</button> }]}
          height={620} onRow={(r) => nav("projects", { id: r.id })} initialSort={["last_activity", -1]} />
      </Card>
      {edit && <EditProject p={edit} onClose={() => { setEdit(null); list.reload(); }} />}
    </div>
  );
}

function EditProject({ p, onClose }: { p: Row; onClose: () => void }) {
  const [ws, setWs] = useState(p.workspace ?? "");
  const [err, setErr] = useState<string | null>(null);
  return (
    <Modal title={`Edit · ${p.name}`} onClose={onClose} foot={<>
      <button className="btn" onClick={() => api.setProjectMeta(p.id, !p.hidden).then(onClose, (e) => setErr(String(e)))}>{p.hidden ? "Unhide" : "Hide from dashboard"}</button>
      <button className="btn primary" onClick={() => api.setProjectMeta(p.id, undefined, ws).then(onClose, (e) => setErr(String(e)))}>Save workspace</button>
    </>}>
      <div className="col">
        <span className="label">Workspace (blank = derived)</span>
        <input className="input" value={ws} onChange={(e) => setWs(e.target.value)} />
        <span className="muted small">Corrections are stored in this app only. Your files are not changed.</span>
        {err && <ErrBox error={err} />}
      </div>
    </Modal>
  );
}

function Evidence({ project, dayStr, workstream, onClose }: { project: string; dayStr?: string; workstream?: string; onClose: () => void }) {
  const ev = useAsync(() => api.timeline({ project, from: dayStr, to: dayStr, workstream, limit: 500 }), [project, dayStr, workstream]);
  return (
    <Modal title={`Evidence · ${dayStr ?? ""} ${workstream ? `· workstream ${workstream}` : ""}`} onClose={onClose}>
      {ev.error && <ErrBox error={ev.error} />}
      <EventList items={ev.data?.items ?? []} />
      {ev.data && !ev.data.items.length && <Empty>No timeline records for this selection (session prompt counts come from transcripts; see the Sessions table)</Empty>}
    </Modal>
  );
}

export function EventList({ items }: { items: Row[] }) {
  return (
    <div className="list">
      {items.map((e) => (
        <div key={e.id} className="li">
          <div className="col" style={{ gap: 3 }}><span className="chip">{e.kind}</span><AgentChip agent={e.agent} /></div>
          <div style={{ minWidth: 0 }}>
            <div className="small">{e.title}</div>
            {e.detail && <div className="dim small clamp3">{e.detail}</div>}
            <div className="mono dim" style={{ fontSize: 10.5 }}>{e.project ? `${e.project} · ` : ""}{e.source} · {e.evidence ? <EvidenceLink e={e} /> : e.ref}</div>
          </div>
          <span className="mono dim small">{String(e.ts).slice(0, 16).replace("T", " ")}</span>
        </div>
      ))}
    </div>
  );
}

function EvidenceLink({ e }: { e: Row }) {
  const file = e.source_file || (String(e.evidence).includes(":\\") ? String(e.evidence).replace(/:\d+$/, "") : null);
  if (file) return <a href="#" onClick={(x) => { x.preventDefault(); api.openPath(file); }}>{String(e.evidence)}</a>;
  return <span>{e.kind === "commit" ? `commit ${short(e.evidence, 10)}` : String(e.evidence)}</span>;
}

function Detail({ id }: { id: string }) {
  const { nav, launch } = useNav();
  const d = useAsync(() => api.project(id), [id], ["projects-updated"]);
  const settings = useAsync(() => api.settings(), []);
  const [range, setRange] = useState<Range>(rangeFor("90d"));
  const act = useAsync(() => api.activity(id, range.from, range.to), [id, range.from, range.to], ["projects-updated"]);
  const r30 = useMemo(() => rangeFor("30d"), []);
  const act30 = useAsync(() => api.activity(id, r30.from, r30.to), [id], ["projects-updated"]);
  const commits = useAsync(() => api.timeline({ project: id, kind: "commit", limit: 2000 }), [id], ["projects-updated"]);
  const docs = useAsync(() => api.timeline({ project: id, limit: 2000 }), [id], ["projects-updated"]);
  const [tab, setTab] = useState("overview");
  const [pick, setPick] = useState<{ day?: string; ws?: string } | null>(null);
  const [session, setSession] = useState<Row | null>(null);
  const aliases: Row = settings.data?.workstream_aliases ?? {};

  const workstreams = useMemo(() => {
    const m = new Map<string, Row>();
    for (const w of d.data?.workstreams ?? []) {
      const name = aliases[w.workstream] ?? w.workstream;
      const cur = m.get(name) ?? { name, raw: [], commits: 0, files: 0 };
      cur.raw.push(w.workstream); cur.commits += w.commits; cur.files += w.files;
      m.set(name, cur);
    }
    return [...m.values()].sort((a, b) => b.commits - a.commits);
  }, [d.data, aliases]);

  if (d.error) return <ErrBox error={d.error} />;
  if (!d.data) return <div className="skeleton" />;
  const { project: p, tasks, sessions, files, agent_files, links, reviews, launches } = d.data;
  const counts: Row = Object.fromEntries(STATUS_ORDER.map((s) => [s, tasks.filter((t: Row) => t.status === s).length]));
  const totalTasks = tasks.length;
  const agentStats = (agent: string) => {
    const ss = sessions.filter((s: Row) => s.agent === agent);
    return {
      sessions: ss.length,
      prompts: ss.reduce((a: number, s: Row) => a + s.prompts, 0),
      edits: ss.reduce((a: number, s: Row) => a + s.edits, 0),
      commits: (commits.data?.items ?? []).filter((c: Row) => c.agent === agent).length,
      handoffs: tasks.filter((t: Row) => t.author === agent && (t.status === "active" || t.status === "pending")).length,
    };
  };
  const cl = agentStats("claude"), cx = agentStats("codex");
  const maxWs = Math.max(1, ...workstreams.map((w) => w.commits));
  const events: Row[] = docs.data?.items ?? [];

  const tabs: [string, string, number | null][] = [
    ["overview", "Dossier", null], ["activity", "Activity", null], ["work", "Workstreams & agents", null],
    ["actions", "Action items", null], ["tasks", "Task board", tasks.length], ["tasklist", "Task list", null], ["commits", "Commits", commits.data?.items.length ?? 0], ["files", "Files", files.length],
    ["agentfiles", "Agent edits", agent_files.length], ["sessions", "Sessions", sessions.length],
    ["decisions", "Decisions", events.filter((e) => e.kind === "decision").length], ["log", "Log", events.filter((e) => e.kind === "log").length],
    ["links", "Related", links.length], ["reviews", "Review", reviews.length], ["launches", "Launches", launches.length],
  ];
  const b = brand(p.workspace);
  const glow = { ["--glow" as string]: b.glow } as React.CSSProperties;

  return (
    <div>
      <div className="row" style={{ marginBottom: 14 }}>
        <button className="btn sm" onClick={() => nav("overview")}>← Dashboard</button>
        <button className="btn sm ghost" onClick={() => nav("projects")}>All projects</button>
      </div>
      <div className="hero" style={{ marginBottom: 26 }}>
        <div style={{ minWidth: 0 }}>
          <div className="row"><span className="label" style={{ color: b.glow }}>{p.workspace}</span><span className="chip">{p.source === "registry" ? "Book of Secrets" : p.source}</span>
            {p.dirty > 0 && <span className="chip warn">{p.dirty} uncommitted</span>}{p.ahead > 0 && <span className="chip warn">{p.ahead} unpushed</span>}</div>
          <h1 style={{ marginTop: 8 }}>{p.name}</h1>
          <div className="when">{p.branch ? `${p.branch} @ ${short(p.head, 7)} · ` : ""}last activity {ago(p.last_activity)}</div>
        </div>
        <div className="board">
          <div><div className="v">{counts.active}</div><div className="k">Open now</div></div>
          <div><div className="v" style={{ color: counts.blocked ? "var(--gold)" : undefined }}>{counts.blocked}</div><div className="k">Blocked</div></div>
          <div><div className="v">{commits.data?.items.length ?? "…"}</div><div className="k">Commits</div></div>
        </div>
      </div>
      <div className="row wrap" style={{ marginBottom: 20 }}><ProjectActions p={p} /></div>
      {p.git_error && <div className="errbox" style={{ marginBottom: 16 }}>git: {p.git_error}</div>}
      <div className="tabs">
        {tabs.map(([k, l, n]) => (
          <button key={k} className={tab === k ? "on" : ""} onClick={() => setTab(k)}>{l}{n != null && <span className="n">{n}</span>}</button>
        ))}
      </div>
      {tab === "overview" && (
        <div className="col" style={{ gap: 22 }}>
        <div className="split">
          <div className="pass" style={{ gridTemplateColumns: "1fr 96px" }}>
            <div className="main-part">
              <div className="row" style={{ justifyContent: "space-between" }}><span className="plabel">Project file · continuity log</span><span className="plabel">{short(p.head, 7)}</span></div>
              <div className="ptitle" style={{ margin: "6px 0 12px" }}>Where it stands</div>
              <div className="txt" style={{ whiteSpace: "pre-wrap" }}>{p.summary ?? "No CONTINUITY LOG summary found."}</div>
              {p.next_action && <div className="line" style={{ cursor: "default", marginTop: 12 }}><span className="gate">Next</span><span className="txt">{String(p.next_action).replace(/^\*\*Next logical (action|task):\*\*\s*/, "")}</span></div>}
              <div className="line" style={{ cursor: "default" }}><span className="gate">Directory</span><span className="txt mono" style={{ fontSize: 11 }}>{p.repo_path ?? p.vault_dir ?? "—"}</span></div>
            </div>
            <div className="stub"><span className="plabel" style={{ writingMode: "vertical-rl", transform: "rotate(180deg)" }}>{p.workspace}</span><Barcode value={p.id} width={70} height={30} /></div>
          </div>
          <section className="card glow" style={glow}>
            <div className="card-head"><span className="label">Task completion</span><div className="right"><span className="dim small">{totalTasks} parsed items</span></div></div>
            <div className="card-body">
              {totalTasks ? (
                <div className="col" style={{ gap: 10 }}>
                  <div style={{ display: "flex", height: 10, borderRadius: 3, overflow: "hidden" }}>
                    {STATUS_ORDER.map((s) => counts[s] ? <div key={s} title={`${STATUS_LABEL[s]}: ${counts[s]}`} style={{ width: `${(100 * counts[s]) / totalTasks}%`, background: STATUS_COLOR[s] }} /> : null)}
                  </div>
                  {STATUS_ORDER.map((s) => (
                    <div key={s} className="row small" style={{ cursor: "pointer" }} onClick={() => setTab("tasks")}>
                      <span style={{ width: 9, height: 9, borderRadius: 2, background: STATUS_COLOR[s], display: "inline-block" }} />
                      <span style={{ flex: 1 }}>{STATUS_LABEL[s]}</span><span className="mono">{counts[s]}</span>
                    </div>
                  ))}
                  <div className="dim small">{counts.done ? `${Math.round((100 * counts.done) / totalTasks)}% of parsed TASKS.md items are checked done.` : "No completion % shown: no items are checked done in TASKS.md."}</div>
                </div>
              ) : <Empty>No TASKS.md found</Empty>}
            </div>
          </section>
        </div>
          <section className="card">
            <div className="card-head"><span className="label">Activity · last 30 days</span><div className="right"><button className="btn sm" onClick={() => setTab("activity")}>Full range</button></div></div>
            <div className="card-body">
              {act30.data ? (
                <DayChart rows={act30.data} from={r30.from} to={r30.to} height={150} onPick={(day) => setPick({ day })}
                  series={[
                    { key: "commits", label: "Commits", color: "var(--violet)" },
                    { key: "claude_prompts", label: "Claude prompts", color: "var(--claude)" },
                    { key: "codex_sessions", label: "Codex threads", color: "var(--codex)" },
                    { key: "log_entries", label: "Log entries", color: "var(--violet-soft)" },
                    { key: "decisions", label: "Decisions", color: "var(--gold)" },
                  ]} />
              ) : <div className="skeleton" />}
            </div>
          </section>
          <div className="split">
            <FolderMap projectId={id} files={files} />
            <div className="col" style={{ gap: 22 }}>
              <section className="card">
                <div className="card-head"><span className="label">Workstreams</span><div className="right"><button className="btn sm" onClick={() => setTab("work")}>Detail</button></div></div>
                <div className="card-body">
                  {workstreams.length ? workstreams.slice(0, 8).map((w) => (
                    <div key={w.name} className="hbar" onClick={() => setPick({ ws: w.raw[0] })} title={`folders: ${w.raw.join(", ")}`}>
                      <span className="mono small ellipsis">{w.name}</span>
                      <div className="track"><div className="fill" style={{ width: `${(100 * w.commits) / maxWs}%` }} /></div>
                      <span className="mono small dim">{w.commits}</span>
                    </div>
                  )) : <Empty>No git history indexed</Empty>}
                </div>
              </section>
              <section className="card">
                <div className="card-head"><span className="label">Agent split</span></div>
                <div className="card-body col" style={{ gap: 8 }}>
                  {[["Sessions", cl.sessions, cx.sessions], ["Commits (trailer)", cl.commits, cx.commits], ["Open tasks tagged", cl.handoffs, cx.handoffs]].map(([label, a, b]) => {
                    const t = Math.max(1, (a as number) + (b as number));
                    return (
                      <div key={label as string}>
                        <div className="row small"><span style={{ flex: 1 }}>{label}</span><span className="mono" style={{ color: "var(--claude)" }}>{a}</span><span className="dim">/</span><span className="mono" style={{ color: "var(--codex)" }}>{b}</span></div>
                        <div style={{ display: "flex", height: 6, borderRadius: 3, overflow: "hidden", background: "rgba(7,5,15,.6)", marginTop: 3 }}>
                          <div style={{ width: `${(100 * (a as number)) / t}%`, background: "var(--claude)" }} />
                          <div style={{ width: `${(100 * (b as number)) / t}%`, background: "var(--codex)" }} />
                        </div>
                      </div>
                    );
                  })}
                  <div className="dim small">Claude {cl.prompts} prompts · {cl.edits} recorded edits. Codex edits aren't recorded.</div>
                </div>
              </section>
            </div>
          </div>
        </div>
      )}
      {tab === "activity" && (
        <Card title="Development activity" right={<RangePicker value={range} onChange={setRange} />}>
          {act.data ? (
            <DayChart rows={act.data} from={range.from} to={range.to} height={240} onPick={(day) => setPick({ day })}
              series={[
                { key: "commits", label: "Commits", color: "var(--violet)" },
                { key: "claude_prompts", label: "Claude prompts", color: "var(--claude)" },
                { key: "codex_sessions", label: "Codex threads", color: "var(--codex)" },
                { key: "log_entries", label: "Log entries", color: "var(--violet-soft)" },
                { key: "decisions", label: "Decisions", color: "var(--gold)" },
              ]} />
          ) : <div className="skeleton" />}
          {act.data && (
            <div className="row wrap small muted" style={{ marginTop: 10, gap: 18 }}>
              <span>Files changed in commits: <b className="mono">{act.data.reduce((a, r) => a + r.files_changed, 0)}</b></span>
              <span>Claude recorded edits: <b className="mono">{act.data.reduce((a, r) => a + r.claude_edits, 0)}</b></span>
              <span>Commits: <b className="mono">{act.data.reduce((a, r) => a + r.commits, 0)}</b></span>
              <span className="dim">Click a bar for the records behind it.</span>
            </div>
          )}
        </Card>
      )}
      {tab === "work" && (
      <div className="grid g2">
        <Card title="Workstreams · from changed paths in git" right={<span className="dim small">rename in Settings › workstream aliases</span>}>
          {workstreams.length ? workstreams.slice(0, 14).map((w) => (
            <div key={w.name} className="hbar" onClick={() => setPick({ ws: w.raw[0] })} title={`folders: ${w.raw.join(", ")} · ${w.files} file changes`}>
              <span className="mono small ellipsis">{w.name}</span>
              <div className="track"><div className="fill" style={{ width: `${(100 * w.commits) / maxWs}%` }} /></div>
              <span className="mono small dim">{w.commits}</span>
            </div>
          )) : <Empty>No git history indexed</Empty>}
        </Card>
        <Card title="Agent contribution">
          <table className="t">
            <thead><tr><th>Evidence</th><th style={{ color: "var(--claude)" }}>Claude</th><th style={{ color: "var(--codex)" }}>Codex</th></tr></thead>
            <tbody>
              <tr><td>Sessions mapped to this project</td><td className="mono">{cl.sessions}</td><td className="mono">{cx.sessions}</td></tr>
              <tr><td>Prompts (transcripts)</td><td className="mono">{cl.prompts}</td><td className="mono dim">not recorded</td></tr>
              <tr><td>Recorded edit tool calls</td><td className="mono">{cl.edits}</td><td className="mono dim">not recorded</td></tr>
              <tr><td>Commits with author/co-author trailer</td><td className="mono">{cl.commits}</td><td className="mono">{cx.commits}</td></tr>
              <tr><td>Open tasks tagged to the agent</td><td className="mono">{cl.handoffs}</td><td className="mono">{cx.handoffs}</td></tr>
            </tbody>
          </table>
          <div className="dim small" style={{ marginTop: 8 }}>Uncommitted changes are not attributed to any agent. Codex per-edit data lives in its rollout files, which are not parsed.</div>
          <div className="row" style={{ marginTop: 10 }}>
            <button className="btn claude" onClick={() => launch(p, "claude", "targeted")}>Targeted Claude task</button>
            <button className="btn codex" onClick={() => launch(p, "codex", "targeted")}>Targeted Codex task</button>
          </div>
        </Card>
      </div>
      )}
      <section className={["overview", "activity", "work"].includes(tab) ? "" : "card"} style={{ overflow: "hidden" }}>
        {tab === "tasks" && <TaskBoard tasks={tasks} project={p} reload={d.reload} />}
        {tab === "actions" && <div className="card-body"><ActionBoard projectId={p.id} /></div>}
        {tab === "tasklist" && <DataTable rows={tasks} height={460} onRow={(r) => api.openPath(r.source_file)} cols={[
          { key: "status", label: "Status", width: "10%", render: (r) => <span className={`chip ${r.status}`}>{r.status}</span>, sort: (r) => STATUS_ORDER.indexOf(r.status) },
          { key: "section", label: "Section", width: "16%" },
          { key: "text", label: "Task", width: "56%" },
          { key: "author", label: "Tag", width: "8%", render: (r) => r.author ? <AgentChip agent={r.author} /> : "—" },
          { key: "line", label: "Line", width: "10%", render: (r) => <span className="mono dim">L{r.line}</span> },
        ]} />}
        {tab === "commits" && <DataTable rows={commits.data?.items ?? []} height={460} initialSort={["ts", -1]} cols={[
          { key: "ts", label: "When", width: "14%", render: (r) => <span className="mono">{String(r.ts).slice(0, 16).replace("T", " ")}</span> },
          { key: "evidence", label: "SHA", width: "9%", render: (r) => <span className="mono">{short(r.evidence, 7)}</span> },
          { key: "title", label: "Subject", width: "45%" },
          { key: "workstream", label: "Workstream", width: "11%" },
          { key: "agent", label: "Agent", width: "9%", render: (r) => <AgentChip agent={r.agent} /> },
          { key: "detail", label: "Author · files", width: "22%" },
        ]} />}
        {tab === "files" && <DataTable rows={files} height={460} initialSort={["commits", -1]} cols={[
          { key: "path", label: "Path", width: "52%", render: (r) => <span className="mono">{r.path}</span> },
          { key: "commits", label: "Commits", width: "10%" }, { key: "added", label: "+", width: "9%" }, { key: "deleted", label: "−", width: "9%" },
          { key: "last_ts", label: "Last change", width: "20%", render: (r) => ago(r.last_ts) },
        ]} />}
        {tab === "agentfiles" && <DataTable rows={agent_files} height={460} cols={[
          { key: "path", label: "Path", width: "56%", render: (r) => <span className="mono">{r.path}</span> },
          { key: "agent", label: "Agent", width: "10%", render: (r) => <AgentChip agent={r.agent} /> },
          { key: "edits", label: "Edit calls", width: "10%" }, { key: "sessions", label: "Sessions", width: "9%" },
          { key: "last_ts", label: "Last", width: "15%", render: (r) => ago(r.last_ts) },
        ]} />}
        {tab === "sessions" && <DataTable rows={sessions} height={460} onRow={setSession} initialSort={["updated", -1]} cols={sessionCols} />}
        {tab === "decisions" && <EventTable rows={events.filter((e) => e.kind === "decision")} />}
        {tab === "log" && <EventTable rows={events.filter((e) => e.kind === "log")} />}
        {tab === "links" && (
          <div className="card-body">
            {links.length ? links.map((l: Row) => (
              <div key={l.target_id} className="li click" onClick={() => nav("projects", { id: l.target_id })}>
                <span className="chip">{l.n}×</span><div><div>{l.name}</div><div className="mono dim small">{String(l.files).split(",").map((f: string) => f.split("\\").slice(-2).join("\\")).join(" · ")}</div></div><span />
              </div>
            )) : <Empty>No explicit references to other projects in this project's continuity files</Empty>}
          </div>
        )}
        {tab === "reviews" && (
          <div className="card-body">
            {reviews.length ? reviews.map((r: Row) => (
              <label key={r.id} className="li"><input type="checkbox" checked={!!r.done} onChange={() => api.toggleReviewTask(r.id).then(d.reload)} /><span className="small">{r.text}</span><span className="dim small">{ago(r.ts)}</span></label>
            )) : <Empty>No review tasks. Create them from Discovery.</Empty>}
          </div>
        )}
        {tab === "launches" && <DataTable rows={launches} height={360} cols={[
          { key: "ts", label: "When", width: "16%", render: (r) => ago(r.ts) }, { key: "agent", label: "Agent", width: "9%", render: (r) => <AgentChip agent={r.agent} /> },
          { key: "mode", label: "Mode", width: "10%" }, { key: "ok", label: "Result", width: "9%", render: (r) => r.ok ? <span className="chip ok">ok</span> : <span className="chip err" title={r.error}>failed</span> },
          { key: "handoff_path", label: "Brief", width: "56%", render: (r) => r.handoff_path ? <a href="#" onClick={(e) => { e.preventDefault(); e.stopPropagation(); api.openPath(r.handoff_path); }}>{r.handoff_path}</a> : "resume" },
        ]} />}
      </section>
      {pick && <Evidence project={id} dayStr={pick.day} workstream={pick.ws} onClose={() => setPick(null)} />}
      {session && <SessionModal s={{ ...session, project_row: p }} onClose={() => setSession(null)} />}
    </div>
  );
}

/** Status board: one column per status, items as cards. Click opens the TASKS.md it came from. */
function TaskBoard({ tasks, project, reload }: { tasks: Row[]; project: Row; reload: () => void }) {
  const [showDone, setShowDone] = useState(false);
  const { launch } = useNav();
  const [tick, setTick] = useState<{ t: Row; pv?: Row; err?: string } | null>(null);
  const openTick = (t: Row) => { setTick({ t }); api.taskTogglePreview(t.id).then((pv) => setTick({ t, pv }), (e) => setTick({ t, err: String(e) })); };
  const confirmTick = () => tick?.pv && api.taskToggle(tick.t.id, tick.pv.before).then(() => { setTick(null); reload(); }, (e) => setTick({ ...tick, err: String(e) }));
  const cols = (["active", "pending", "blocked", "deferred", "done"] as const).filter((s) => s !== "done" || showDone);
  if (!tasks.length) return <div className="card-body"><Empty>No TASKS.md items for this project</Empty></div>;
  return (
    <div className="card-body">
      <div className="row" style={{ marginBottom: 14 }}>
        <div style={{ flex: 1 }}><StatusBar counts={Object.fromEntries(STATUS_ORDER.map((st) => [st, tasks.filter((t) => t.status === st).length]))} height={8} legend /></div>
        <label className="row small muted"><input type="checkbox" checked={showDone} onChange={(e) => setShowDone(e.target.checked)} /> show done</label>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: `repeat(${cols.length}, minmax(0, 1fr))`, gap: 14 }}>
        {cols.map((st) => {
          const items = tasks.filter((t) => t.status === st);
          return (
            <div key={st} className="col" style={{ gap: 8 }}>
              <div className="row" style={{ borderBottom: `2px solid ${STATUS_COLOR[st]}`, paddingBottom: 6 }}>
                <span className="label" style={{ color: STATUS_COLOR[st] }}>{STATUS_LABEL[st]}</span>
                <span className="mono dim" style={{ marginLeft: "auto" }}>{items.length}</span>
              </div>
              <div className="col" style={{ gap: 8, maxHeight: 520, overflow: "auto", paddingRight: 2 }}>
                {items.map((t) => (
                  <div key={t.id} className="card" style={{ padding: "10px 12px", cursor: "pointer", borderLeft: `3px solid ${STATUS_COLOR[st]}` }} onClick={() => api.openPath(t.source_file)} title={`${t.source_file}:${t.line}`}>
                    <div className="small clamp3">{t.text}</div>
                    <div className="row" style={{ marginTop: 6, gap: 6 }}>
                      {t.author && <AgentChip agent={t.author} />}
                      <span className="dim mono ellipsis" style={{ fontSize: 9.5 }}>{t.section || "—"} · L{t.line}</span>
                    </div>
                    <div className="row" style={{ marginTop: 8, gap: 6 }} onClick={(e) => e.stopPropagation()}>
                      <button className="btn sm" onClick={() => openTick(t)} title="Tick or untick this item in TASKS.md (preview first)">{st === "done" ? "Reopen" : "Done ✓"}</button>
                      {st !== "done" && <button className="btn sm claude" onClick={() => launch(project, "claude", "custom", undefined, t.text)} title="Open Claude Code on this task">Claude</button>}
                      {st !== "done" && <button className="btn sm codex" onClick={() => launch(project, "codex", "custom", undefined, t.text)} title="Open Codex on this task">Codex</button>}
                    </div>
                  </div>
                ))}
                {!items.length && <div className="dim small mono" style={{ padding: 8 }}>none</div>}
              </div>
            </div>
          );
        })}
      </div>
      {tick && (
        <Modal title="Update TASKS.md" onClose={() => setTick(null)} foot={<button className="btn primary" disabled={!tick.pv} onClick={confirmTick}>Write change</button>}>
          {tick.err ? <ErrBox error={tick.err} /> : !tick.pv ? <div className="skeleton" /> : (
            <div className="col">
              <div className="dim small mono">{String(tick.pv.file)} · line {tick.pv.line}</div>
              <pre className="doc" style={{ color: "var(--muted)" }}>− {tick.pv.before}</pre>
              <pre className="doc">+ {tick.pv.after}</pre>
              <div className="dim small">Only this line changes. Agents and the vault see it immediately.</div>
            </div>
          )}
        </Modal>
      )}
    </div>
  );
}

export const sessionCols: Col[] = [
  { key: "updated", label: "Last write", width: "12%", render: (r) => ago(r.updated) },
  { key: "agent", label: "Agent", width: "8%", render: (r) => <AgentChip agent={r.agent} /> },
  { key: "title", label: "Title", width: "40%" },
  { key: "state", label: "State", width: "11%", render: (r) => { const s = sessionState(r.updated); return <span className={`chip ${s.cls}`}>{r.archived ? "archived" : s.label}</span>; }, sort: (r) => r.updated },
  { key: "prompts", label: "Prompts", width: "8%", render: (r) => r.agent === "claude" ? r.prompts : "—" },
  { key: "edits", label: "Edits", width: "7%", render: (r) => r.agent === "claude" ? r.edits : "—" },
  { key: "map_method", label: "Mapped by", width: "14%", render: (r) => <span className="dim small">{r.map_method ?? "unmapped"}</span> },
];

export function SessionModal({ s, onClose }: { s: Row; onClose: () => void }) {
  const files = useAsync(() => api.sessionFiles(s.id), [s.id]);
  const { launch, nav } = useNav();
  return (
    <Modal title={`${s.agent} session · ${short(s.id, 12)}`} onClose={onClose} foot={<>
      <button className="btn" onClick={() => { onClose(); nav("chat", { id: s.id }); }}>Open in CHAT</button>
      {s.path && <button className="btn" onClick={() => api.openPath(String(s.path).replace(/[\\/][^\\/]+$/, ""))}>Open transcript folder</button>}
      {s.project_row && <button className="btn primary" onClick={() => { onClose(); launch(s.project_row, s.agent, "continue"); }}>Continue this project</button>}
    </>}>
      <div className="col">
        <div className="display" style={{ fontSize: 15 }}>{s.title}</div>
        <div className="mono dim small">id {s.id} · cwd {s.cwd ?? "—"} · branch {s.branch ?? "—"}</div>
        <div className="mono dim small">started {s.started} · last write {s.updated} · mapped by {s.map_method ?? "—"}</div>
        <div className="label" style={{ marginTop: 8 }}>Recorded file edits</div>
        {files.data?.length ? (
          <div className="list">{files.data.map((f) => <div key={f.path} className="li"><span className="chip">{f.edits}×</span><span className="mono small ellipsis">{f.path}</span><span className="dim small">{ago(f.last_ts)}</span></div>)}</div>
        ) : <Empty>{s.agent === "codex" ? "Codex edit data is not indexed" : "No edit tool calls recorded"}</Empty>}
      </div>
    </Modal>
  );
}

function EventTable({ rows }: { rows: Row[] }) {
  const [open, setOpen] = useState<Row | null>(null);
  return (
    <>
      <DataTable rows={rows} height={460} onRow={setOpen} initialSort={["ts", -1]} cols={[
        { key: "ts", label: "Date", width: "11%", render: (r) => <span className="mono">{r.ts || "undated"}</span> },
        { key: "title", label: "Entry", width: "59%" },
        { key: "agent", label: "Author", width: "10%", render: (r) => <AgentChip agent={r.agent} /> },
        { key: "evidence", label: "Source", width: "20%", render: (r) => <span className="mono dim">{String(r.evidence).split("\\").pop()}</span> },
      ]} />
      {open && (
        <Modal title={open.title} onClose={() => setOpen(null)} foot={<button className="btn" onClick={() => api.openPath(open.source_file)}>Open source file</button>}>
          <pre className="doc">{open.detail}</pre>
          <div className="mono dim small" style={{ marginTop: 8 }}>{open.evidence}</div>
        </Modal>
      )}
    </>
  );
}

export default function Projects() {
  const { params } = useNav();
  return params.id ? <Detail id={params.id} /> : <ProjectList />;
}
