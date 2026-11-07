import { lazy, Suspense, useEffect, useState } from "react";
import { api, on, onConnection, PHONE, phonePaired, type LaunchReq, type Row } from "./api";
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
const Chat = lazy(() => import("./views/Chat"));

// PapertrailOS bracketed bottom navigation.
const NAV: [View, string][] = [
  ["overview", "OVERVIEW"],
  ["projects", "PROJECTS"],
  ["actions", "ACTIONS"],
  ["chat", "CHAT"],
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
    <div className="search" style={{ position: "relative" }}>
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

// Phone (D027): read views only. Anything else stays on the PC.
const PHONE_VIEWS: View[] = ["overview", "workspace", "projects", "chat", "agents", "timeline"];

function Unpaired() {
  return (
    <div className="login"><div className="login-panel" style={{ textAlign: "center" }}>
      <div className="display" style={{ fontSize: 18, letterSpacing: ".2em" }}>Pair this phone</div>
      <div className="small">On the PC open <b>Settings › Phone access</b>, choose <b>Pair a phone</b>, then scan the QR code with this phone's camera.</div>
      <div className="dim small">Tailscale must be on, on this phone and on the PC.</div>
    </div></div>
  );
}

/** Phone: a slim bar while the PC can't be reached. The page keeps showing what it had; calls resume on their own. */
function Reconnecting() {
  const [since, setSince] = useState(0);
  const [, tick] = useState(0);
  useEffect(() => onConnection(setSince), []);
  useEffect(() => { if (!since) return; const t = setInterval(() => tick((n) => n + 1), 1000); return () => clearInterval(t); }, [since]);
  if (!since) return null;
  const s = Math.round((Date.now() - since) / 1000);
  return (
    <div className="reconnect" role="status">
      <span className="dot busy" />Reconnecting to your PC…{s >= 15 ? " Check that Tailscale is on, on this phone and on the PC." : ""}
    </div>
  );
}

// Phone: where you were, so a tab Android reloads in the background reopens on the same page or chat.
const SPOT = "pt-phone-spot";
const savedSpot = (): { view: View; params: Row } | null => {
  if (!PHONE) return null;
  try { const s = JSON.parse(localStorage.getItem(SPOT) ?? "null"); return s && PHONE_VIEWS.includes(s.view) ? s : null; } catch { return null; }
};

// Errors surface in the status bar, never as blocking browser dialogs.
let alertless: (m: string) => void = () => {};

export default function App() {
  const [view, setView] = useState<View>(() => savedSpot()?.view ?? "overview");
  // undefined = checking; null = not signed in (login shows until signed in or skipped)
  const [me, setMe] = useState<Row | null | undefined>(undefined);
  // Solo use for now (owner 2026-11-03): no sign-in screen at launch; "Sign in" in the top bar still opens it.
  const [gate, setGate] = useState(false);
  useEffect(() => { api.team().then((t) => { setMe(t.me); if (t.me) setGate(false); }, () => setMe(null)); }, []);
  const [params, setParams] = useState<Row>(() => savedSpot()?.params ?? {});
  useEffect(() => { if (PHONE) localStorage.setItem(SPOT, JSON.stringify({ view, params })); }, [view, params]);
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
  const [unpaired, setUnpaired] = useState(PHONE && !phonePaired());
  useEffect(() => { const f = () => setUnpaired(true); window.addEventListener("pt-unpaired", f); return () => window.removeEventListener("pt-unpaired", f); }, []);
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
  const [offline, setOffline] = useState(0);
  useEffect(() => (PHONE ? onConnection(setOffline) : undefined), []);
  const blocked = PHONE && !PHONE_VIEWS.includes(view);
  const page = blocked ? <div className="empty">This page is on the PC only. The phone is read-only.</div> : { overview: <Overview />, workspace: <Workspace />, projects: <Projects />, actions: <Actions />, chat: <Chat />, agents: <Agents />, launchpad: <Launchpad />, graph: <Graph />, discovery: <Discovery />, timeline: <Timeline />, settings: <Settings /> }[view];

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
          {PHONE ? null : me ? (
            <span className="userchip" title="Signed in: your name goes on action items">
              <span className="avatar" style={{ background: me.color }}>{String(me.name)[0]}</span>{me.name}
              <button className="btn sm ghost" onClick={() => api.logout().then(() => { setMe(null); setGate(true); })}>Lock</button>
            </span>
          ) : <button className="btn sm" onClick={() => setGate(true)}>Sign in</button>}
          <button className="btn sm desk-only" onClick={() => api.toggleChat().catch((e) => alertless(String(e)))} title="Slide-out chat (Discord by default; set the URL in Settings)">Chat</button>
          <button className="btn sm desk-only" onClick={() => api.refresh()} disabled={busy} title="Re-index projects, git and sessions">Refresh</button>
          <div className="sysstat">
            <div className="label" style={{ fontSize: 9 }}>System status</div>
            <div className="v">
              <span className={`dot ${busy || offline ? "busy" : status.state === "error" ? "err" : ""}`} />
              {offline ? "RECONNECTING" : busy ? `${String(status.stage ?? status.state).toUpperCase()}${status.total ? ` ${status.done}/${status.total}` : ""}` : status.state === "error" ? "DEGRADED" : "ONLINE"}
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
            {NAV.filter(([k]) => !PHONE || PHONE_VIEWS.includes(k)).map(([k, label]) => (
              <button key={k} className={view === k ? "on" : ""} onClick={() => nav(k)}>{label}</button>
            ))}
            <span className="br">]</span>
          </nav>
        </footer>
      </div>
      {PHONE && <Reconnecting />}
      {unpaired && <Unpaired />}
      {gate && me !== undefined && <Login onDone={(m) => { setMe(m); setGate(false); }} />}
      {launching && <LaunchDialog project={launching.p} agent={launching.agent} mode={launching.mode} preset={launching.preset} task={launching.task} onClose={() => setLaunching(null)} />}
    </NavCtx.Provider>
  );
}
