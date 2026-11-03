import { lazy, Suspense, useEffect, useState } from "react";
import { api, on, type LaunchReq, type Row } from "./api";
import { ago, Barcode, Boundary, LaunchDialog, NavCtx, type View } from "./ui";
import Overview from "./views/Overview";
import Login from "./Login";

// Heavy views load only when opened.
const Projects = lazy(() => import("./views/Projects"));
const Workspace = lazy(() => import("./views/Workspace"));
const Agents = lazy(() => import("./views/Agents"));
const Launchpad = lazy(() => import("./views/Launchpad"));
const Graph = lazy(() => import("./views/Graph"));
const Discovery = lazy(() => import("./views/Discovery"));
const Timeline = lazy(() => import("./views/Timeline"));
const Settings = lazy(() => import("./views/Settings"));
const Actions = lazy(() => import("./views/Actions"));

// PapertrailOS bracketed bottom navigation.
const NAV: [View, string][] = [
  ["overview", "OVERVIEW"],
  ["projects", "PROJECTS"],
  ["actions", "ACTIONS"],
  ["agents", "AGENTS"],
  ["launchpad", "LAUNCHPAD"],
  ["graph", "THE GRAPH"],
  ["discovery", "DISCOVERY"],
  ["timeline", "TIMELINE"],
  ["settings", "SETTINGS"],
];

function Search({ go }: { go: (v: View, p: Row) => void }) {
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<Row[]>([]);
  useEffect(() => {
    if (q.trim().length < 2) return setHits([]);
    const t = setTimeout(() => api.search(q).then(setHits, () => setHits([])), 180);
    return () => clearTimeout(t);
  }, [q]);
  return (
    <div style={{ position: "relative" }}>
      <input className="input" style={{ width: 280 }} placeholder="Search the paper trail…" value={q} onChange={(e) => setQ(e.target.value)} />
      {hits.length > 0 && (
        <div className="card" style={{ position: "absolute", top: 38, right: 0, width: 480, maxHeight: 440, overflow: "auto", zIndex: 30, padding: "4px 14px" }}>
          {hits.map((h, i) => (
            <div key={i} className="li click" onClick={() => { setQ(""); go(h.kind === "project" ? "projects" : "timeline", h.kind === "project" ? { id: h.project_id } : { project: h.project_id, q }); }}>
              <span className="chip">{h.kind}</span>
              <span className="small">{h.snip}</span>
              <span className="dim small">{h.project}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// Errors surface in the status bar, never as blocking browser dialogs.
let alertless: (m: string) => void = () => {};

export default function App() {
  const [view, setView] = useState<View>("overview");
  // undefined = checking; null = not signed in (login shows until signed in or skipped)
  const [me, setMe] = useState<Row | null | undefined>(undefined);
  const [gate, setGate] = useState(true);
  useEffect(() => { api.team().then((t) => { setMe(t.me); if (t.me) setGate(false); }, () => setMe(null)); }, []);
  const [params, setParams] = useState<Row>({});
  const [status, setStatus] = useState<Row>({ state: "idle" });
  const [lastRefresh, setLastRefresh] = useState<string | null>(null);
  const [launching, setLaunching] = useState<{ p: Row; agent?: "claude" | "codex"; mode?: LaunchReq["mode"]; preset?: string; task?: string } | null>(null);

  useEffect(() => {
    api.status().then((s) => { setStatus(s.status); setLastRefresh(s.last_refresh); }, () => {});
    const off = on("index-status", (s: Row) => {
      setStatus(s);
      if (s.state === "idle") setLastRefresh(s.at);
    });
    return () => { off.then((f) => f()); };
  }, []);

  const [notice, setNotice] = useState<string | null>(null);
  alertless = setNotice;
  const nav = (v: View, p: Row = {}) => { setView(v); setParams(p); };
  // From a project page, Escape or the mouse "back" button returns to the dashboard home (the map).
  useEffect(() => {
    if (!((view === "projects" && params.id) || view === "workspace")) return;
    const key = (e: KeyboardEvent) => { if (e.key === "Escape" && !document.querySelector(".scrim")) nav("overview"); };
    const mouse = (e: MouseEvent) => { if (e.button === 3) { e.preventDefault(); nav("overview"); } };
    window.addEventListener("keydown", key);
    window.addEventListener("mouseup", mouse);
    return () => { window.removeEventListener("keydown", key); window.removeEventListener("mouseup", mouse); };
  }, [view, params]);
  const launch = (p: Row, agent?: "claude" | "codex", mode?: LaunchReq["mode"], preset?: string, task?: string) => setLaunching({ p, agent, mode, preset, task });
  const busy = status.state === "indexing" || status.state === "discovery";
  const page = { overview: <Overview />, workspace: <Workspace />, projects: <Projects />, actions: <Actions />, agents: <Agents />, launchpad: <Launchpad />, graph: <Graph />, discovery: <Discovery />, timeline: <Timeline />, settings: <Settings /> }[view];

  return (
    <NavCtx.Provider value={{ nav, params, launch }}>
      <div className="backdrop" />
      <div className="shell">
        <header className="topbar">
          <div className="wordmark" onClick={() => nav("overview")}>
            <svg width="24" height="24" viewBox="0 0 24 24" aria-hidden><path d="M2 11 22 2l-6 20-4-8z" fill="var(--fg)" /><path d="m12 14 10-12-13 15z" fill="var(--violet-soft)" /></svg>
            <div>
              <div className="name">PAPERTRAIL // COMMAND CENTER</div>
              <div className="tag">TRACK. CONNECT. REMEMBER.</div>
            </div>
          </div>
          <div className="spacer" />
          <Search go={nav} />
          {me ? (
            <span className="userchip" title="Signed in: your name goes on action items">
              <span className="avatar" style={{ background: me.color }}>{String(me.name)[0]}</span>{me.name}
              <button className="btn sm ghost" onClick={() => api.logout().then(() => { setMe(null); setGate(true); })}>Lock</button>
            </span>
          ) : <button className="btn sm" onClick={() => setGate(true)}>Sign in</button>}
          <button className="btn sm" onClick={() => api.toggleChat().catch((e) => alertless(String(e)))} title="Slide-out chat (Discord by default; set the URL in Settings)">Chat</button>
          <button className="btn sm" onClick={() => api.refresh()} disabled={busy} title="Re-index projects, git and sessions">Refresh</button>
          <div className="sysstat">
            <div className="label" style={{ fontSize: 9 }}>System status</div>
            <div className="v">
              <span className={`dot ${busy ? "busy" : status.state === "error" ? "err" : ""}`} />
              {busy ? `${String(status.stage ?? status.state).toUpperCase()}${status.total ? ` ${status.done}/${status.total}` : ""}` : status.state === "error" ? "DEGRADED" : "ONLINE"}
            </div>
          </div>
          <div className="ptbadge">PT</div>
        </header>
        <main className="main">
          <Boundary name={view} key={view + JSON.stringify(params)}>
            <Suspense fallback={<div className="skeleton" />}>{page}</Suspense>
          </Boundary>
        </main>
        <footer className="bottombar">
          <span>PAPERTRAIL CC v0.1.0</span>
          <Barcode value="papertrail-command-center" width={110} height={14} />
          <span>INDEXED {lastRefresh ? ago(lastRefresh).toUpperCase() : "PENDING"}</span>
          {status.state === "error" && <span style={{ color: "var(--signal)" }}>{String(status.message)}</span>}
          {notice && <span style={{ color: "var(--signal)", cursor: "pointer" }} onClick={() => setNotice(null)}>{notice} ✕</span>}
          <nav className="bnav">
            <span className="br">[</span>
            {NAV.map(([k, label]) => (
              <button key={k} className={view === k ? "on" : ""} onClick={() => nav(k)}>{label}</button>
            ))}
            <span className="br">]</span>
          </nav>
        </footer>
      </div>
      {gate && me !== undefined && <Login onDone={(m) => { setMe(m); setGate(false); }} />}
      {launching && <LaunchDialog project={launching.p} agent={launching.agent} mode={launching.mode} preset={launching.preset} task={launching.task} onClose={() => setLaunching(null)} />}
    </NavCtx.Provider>
  );
}
