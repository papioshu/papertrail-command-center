import { useMemo, useRef, useState } from "react";
import { api, type Row } from "../api";
import { ago, Card, Empty, ErrBox, useAsync, useNav } from "../ui";

/* Deterministic radial layout, computed once per data/expansion change. No physics, no animation loop:
   the SVG only re-renders on interaction. Hub = Book of Secrets, ring 1 = workspaces, ring 2 = projects,
   ring 3 = workstreams (from git paths) for expanded projects. Dashed lines = explicit cross-references
   found in continuity files. */
type N = { id: string; kind: "hub" | "ws" | "project" | "stream"; label: string; x: number; y: number; r: number; row?: Row; parent?: string };
type E = { a: string; b: string; kind: "tree" | "link"; n?: number; files?: string };

const R1 = 210, R2 = 410, R3 = 110;

function layout(projects: Row[], streams: Row[], links: Row[], expanded: Set<string>) {
  const nodes: N[] = [{ id: "hub", kind: "hub", label: "BOOK OF SECRETS", x: 0, y: 0, r: 22 }];
  const edges: E[] = [];
  const groups = new Map<string, Row[]>();
  for (const p of projects) groups.set(p.workspace, [...(groups.get(p.workspace) ?? []), p]);
  const order = [...groups.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  const weight = (p: Row) => 1 + (expanded.has(p.id) ? Math.min(6, streams.filter((s) => s.project_id === p.id).length) * 0.35 : 0);
  const total = order.reduce((a, [, ps]) => a + ps.reduce((x, p) => x + weight(p), 0), 0) || 1;
  let theta = -Math.PI / 2;
  for (const [ws, ps] of order) {
    const span = (2 * Math.PI * ps.reduce((x, p) => x + weight(p), 0)) / total;
    const mid = theta + span / 2;
    const multi = ps.length > 1;
    if (multi) {
      nodes.push({ id: `ws:${ws}`, kind: "ws", label: ws, x: R1 * Math.cos(mid), y: R1 * Math.sin(mid), r: 11 });
      edges.push({ a: "hub", b: `ws:${ws}`, kind: "tree" });
    }
    let t = theta;
    for (const p of ps) {
      const w = (2 * Math.PI * weight(p)) / total;
      const a = t + w / 2;
      const rad = multi ? R2 : (R1 + R2) / 2;
      const recent = p.last_activity ? (Date.now() - Date.parse(p.last_activity.length === 10 ? p.last_activity + "T12:00:00" : p.last_activity)) / 864e5 : 999;
      nodes.push({ id: p.id, kind: "project", label: p.name, x: rad * Math.cos(a), y: rad * Math.sin(a), r: 6 + Math.min(8, Math.sqrt(p.active || 0) * 2), row: { ...p, recent } });
      edges.push({ a: multi ? `ws:${ws}` : "hub", b: p.id, kind: "tree" });
      if (expanded.has(p.id)) {
        const ss = streams.filter((s) => s.project_id === p.id).sort((x, y) => y.commits - x.commits).slice(0, 6);
        ss.forEach((s, i) => {
          const sa = a + (i - (ss.length - 1) / 2) * Math.min(0.09, w / Math.max(1, ss.length));
          const id = `st:${p.id}:${s.workstream}`;
          nodes.push({ id, kind: "stream", label: s.workstream, x: (rad + R3) * Math.cos(sa), y: (rad + R3) * Math.sin(sa), r: 3 + Math.min(5, Math.log2(1 + s.commits)), parent: p.id, row: s });
          edges.push({ a: p.id, b: id, kind: "tree" });
        });
      }
      t += w;
    }
    theta += span;
  }
  const ids = new Set(nodes.map((n) => n.id));
  for (const l of links) if (ids.has(l.project_id) && ids.has(l.target_id)) edges.push({ a: l.project_id, b: l.target_id, kind: "link", n: l.n, files: l.files });
  return { nodes, edges };
}

function color(n: N) {
  if (n.kind === "hub") return "var(--violet)";
  if (n.kind === "ws") return "var(--violet-soft)";
  if (n.kind === "stream") return "var(--line-2)";
  const d = n.row?.recent ?? 999;
  return d < 7 ? "var(--violet-glow)" : d < 30 ? "var(--violet-soft)" : "var(--dim)";
}

export default function Graph() {
  const { nav, params, launch } = useNav();
  const g = useAsync(() => api.graph(), [], ["projects-updated"]);
  const plist = useAsync(() => api.projects(true), [], ["projects-updated"]);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(params.focus ? [params.focus] : []));
  const [sel, setSel] = useState<string | null>(params.focus ?? null);
  const [q, setQ] = useState("");
  const [ws, setWs] = useState("");
  const [unsorted, setUnsorted] = useState(false);
  const [focusMode, setFocusMode] = useState(!!params.focus);
  const [view, setView] = useState({ x: 0, y: 0, k: 0.85 });
  const [moved, setMoved] = useState<Record<string, { x: number; y: number }>>({});
  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<{ mode: "pan" | "node"; id?: string; sx: number; sy: number; ox: number; oy: number; moved: boolean } | null>(null);

  const projects: Row[] = useMemo(
    () => (g.data?.projects ?? []).filter((p: Row) => (unsorted || p.workspace !== "UNSORTED") && (!ws || p.workspace === ws)),
    [g.data, unsorted, ws],
  );
  const { nodes, edges } = useMemo(
    () => layout(projects, g.data?.workstreams ?? [], g.data?.links ?? [], expanded),
    [projects, g.data, expanded],
  );
  const pos = (n: N) => moved[n.id] ?? n;
  const byId = useMemo(() => new Map(nodes.map((n) => [n.id, n])), [nodes]);
  const neighbors = useMemo(() => {
    if (!sel || !focusMode) return null;
    const s = new Set([sel]);
    for (const e of edges) {
      if (e.a === sel) s.add(e.b);
      if (e.b === sel) s.add(e.a);
    }
    return s;
  }, [sel, focusMode, edges]);
  const ql = q.trim().toLowerCase();
  const match = (n: N) => !!ql && n.label.toLowerCase().includes(ql);

  const onWheel = (e: React.WheelEvent) => {
    const r = svg.current!.getBoundingClientRect();
    const k = Math.min(3, Math.max(0.25, view.k * (e.deltaY < 0 ? 1.12 : 1 / 1.12)));
    const mx = e.clientX - r.left - r.width / 2, my = e.clientY - r.top - r.height / 2;
    setView({ k, x: mx - ((mx - view.x) * k) / view.k, y: my - ((my - view.y) * k) / view.k });
  };
  const down = (e: React.PointerEvent, id?: string) => {
    e.stopPropagation();
    (e.target as Element).setPointerCapture?.(e.pointerId);
    const n = id ? byId.get(id) : undefined;
    const p = n ? pos(n) : view;
    drag.current = { mode: id ? "node" : "pan", id, sx: e.clientX, sy: e.clientY, ox: p.x, oy: p.y, moved: false };
  };
  const move = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.sx, dy = e.clientY - d.sy;
    if (Math.abs(dx) + Math.abs(dy) > 3) d.moved = true;
    if (d.mode === "pan") setView({ ...view, x: d.ox + dx, y: d.oy + dy });
    else if (d.id && d.moved) setMoved({ ...moved, [d.id]: { x: d.ox + dx / view.k, y: d.oy + dy / view.k } });
  };
  const up = () => {
    const d = drag.current;
    drag.current = null;
    if (d?.mode === "node" && !d.moved && d.id) setSel(d.id);
    if (d?.mode === "pan" && !d.moved) setSel(null);
  };
  const center = (n: N) => { const p = pos(n); setView({ ...view, x: -p.x * view.k, y: -p.y * view.k }); };
  const toggle = (id: string) => { const s = new Set(expanded); s.has(id) ? s.delete(id) : s.add(id); setExpanded(s); };

  const selected = sel ? byId.get(sel) : undefined;
  const selProject = selected?.kind === "project" ? plist.data?.find((p) => p.id === selected.id) : selected?.kind === "stream" ? plist.data?.find((p) => p.id === selected.parent) : undefined;
  const selLinks = selected ? edges.filter((e) => e.kind === "link" && (e.a === selected.id || e.b === selected.id)) : [];
  const workspaces = [...new Set((g.data?.projects ?? []).map((p: Row) => p.workspace))].sort() as string[];

  if (g.error) return <ErrBox error={g.error} />;
  return (
    <div className="col" style={{ gap: 12 }}>
      <div className="page-head" style={{ marginBottom: 0 }}>
        <div>
          <h1>The Graph</h1>
          <div className="sub">Book of Secrets → workspaces → projects → workstreams · dashed = explicit cross-references</div>
        </div>
        <div className="right">
          <input className="input" placeholder="Search nodes" value={q} onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") { const n = nodes.find(match); if (n) { setSel(n.id); center(n); } } }} style={{ width: 170 }} />
          <select className="input" value={ws} onChange={(e) => setWs(e.target.value)}>
            <option value="">All workspaces</option>
            {workspaces.map((w) => <option key={w} value={w}>{w}</option>)}
          </select>
          <label className="row small muted"><input type="checkbox" checked={unsorted} onChange={(e) => setUnsorted(e.target.checked)} /> unsorted</label>
          <label className="row small muted"><input type="checkbox" checked={focusMode} onChange={(e) => setFocusMode(e.target.checked)} /> focus selection</label>
          <button className="btn sm" onClick={() => setExpanded(new Set(projects.map((p) => p.id)))}>Expand all</button>
          <button className="btn sm" onClick={() => setExpanded(new Set())}>Collapse</button>
          <button className="btn sm" onClick={() => { setView({ x: 0, y: 0, k: 0.85 }); setMoved({}); }}>Reset</button>
        </div>
      </div>
      <div className="graph-wrap">
        {!projects.length && !g.loading && <Empty>No projects to graph</Empty>}
        <svg ref={svg} className={drag.current?.mode === "pan" ? "panning" : ""} onWheel={onWheel} onPointerDown={(e) => down(e)} onPointerMove={move} onPointerUp={up} onPointerLeave={up}>
          <g transform={`translate(${(svg.current?.clientWidth ?? 1000) / 2 + view.x},${(svg.current?.clientHeight ?? 700) / 2 + view.y}) scale(${view.k})`}>
            <circle r={R1} fill="none" stroke="var(--ink-2)" />
            <circle r={R2} fill="none" stroke="var(--ink-2)" strokeDasharray="2 6" />
            {edges.map((e, i) => {
              const a = byId.get(e.a), b = byId.get(e.b);
              if (!a || !b) return null;
              const pa = pos(a), pb = pos(b);
              const dim = neighbors && !(neighbors.has(e.a) && neighbors.has(e.b));
              if (e.kind === "link") {
                const mx = (pa.x + pb.x) / 2 * 0.55, my = (pa.y + pb.y) / 2 * 0.55;
                return <path key={i} d={`M${pa.x},${pa.y} Q${mx},${my} ${pb.x},${pb.y}`} fill="none" stroke="var(--violet)" strokeOpacity={dim ? 0.08 : 0.55} strokeDasharray="4 4" strokeWidth={Math.min(3, 0.8 + Math.log2(1 + (e.n ?? 1)) / 2)} />;
              }
              return <line key={i} x1={pa.x} y1={pa.y} x2={pb.x} y2={pb.y} stroke="var(--line-2)" strokeOpacity={dim ? 0.12 : 0.7} />;
            })}
            {nodes.map((n) => {
              const p = pos(n);
              const dim = neighbors && !neighbors.has(n.id);
              const hl = match(n) || sel === n.id;
              const outer = Math.hypot(p.x, p.y) > 1;
              const ang = Math.atan2(p.y, p.x);
              const right = Math.cos(ang) >= 0;
              return (
                <g key={n.id} className="node" transform={`translate(${p.x},${p.y})`} opacity={dim ? 0.18 : 1}
                  onPointerDown={(e) => down(e, n.id)} onDoubleClick={() => n.kind === "project" && toggle(n.id)}>
                  {n.kind === "hub" ? (
                    <rect x={-n.r * 1.4} y={-n.r * 0.8} width={n.r * 2.8} height={n.r * 1.6} fill="var(--panel)" stroke="var(--violet)" strokeWidth={hl ? 2.5 : 1.5} />
                  ) : n.kind === "ws" ? (
                    <rect x={-n.r} y={-n.r} width={n.r * 2} height={n.r * 2} transform="rotate(45)" fill="var(--ink)" stroke={color(n)} strokeWidth={hl ? 2.5 : 1.2} />
                  ) : (
                    <circle r={n.r} fill={n.kind === "stream" ? "var(--ink)" : color(n)} stroke={hl ? "#fff" : n.kind === "stream" ? "var(--violet-soft)" : "var(--ink)"} strokeWidth={hl ? 2 : 1} />
                  )}
                  {n.kind === "hub" ? (
                    <text textAnchor="middle" y={4} style={{ fontSize: 10, letterSpacing: "0.2em" }}>BOOK OF SECRETS</text>
                  ) : (
                    <text x={outer ? (right ? n.r + 6 : -n.r - 6) : 0} y={4} textAnchor={right ? "start" : "end"}
                      style={{ fontSize: n.kind === "stream" ? 9 : n.kind === "ws" ? 10.5 : 10, fill: n.kind === "stream" ? "var(--muted)" : n.kind === "ws" ? "var(--violet-soft)" : undefined }}>
                      {n.kind === "ws" ? n.label.toUpperCase() : n.label}
                    </text>
                  )}
                </g>
              );
            })}
          </g>
        </svg>
        <div className="graph-side">
          {selected && selected.kind !== "hub" && (
            <Card title={selected.kind === "ws" ? "Workspace" : selected.kind === "stream" ? "Workstream" : "Project"}>
              <div className="col">
                <div className="display" style={{ fontSize: 15 }}>{selected.label}</div>
                {selected.kind === "ws" && <div className="muted small">{nodes.filter((n) => n.kind === "project" && edges.some((e) => e.a === selected.id && e.b === n.id)).length} projects</div>}
                {selected.kind === "stream" && <div className="muted small">{selected.row?.commits} commits touch <span className="mono">{selected.label}</span> in {selProject?.name}. Derived from git paths.</div>}
                {selProject && selected.kind === "project" && (
                  <>
                    <div className="mono small dim">{selProject.repo_path || selProject.vault_dir}</div>
                    <div className="small">Last activity {ago(selProject.last_activity)} · {selProject.active_tasks} open · {selProject.blocked_tasks} blocked</div>
                    {selProject.summary && <div className="small muted clamp3">{selProject.summary}</div>}
                    {selLinks.length > 0 && (
                      <div className="col" style={{ gap: 4 }}>
                        <span className="label">Referenced with</span>
                        {selLinks.map((e, i) => {
                          const other = byId.get(e.a === selected.id ? e.b : e.a);
                          return <div key={i} className="small" style={{ cursor: "pointer" }} title={e.files} onClick={() => other && (setSel(other.id), center(other))}>↔ {other?.label} <span className="dim">({e.n}× · {e.a === selected.id ? "this project's docs" : "their docs"})</span></div>;
                        })}
                      </div>
                    )}
                  </>
                )}
                {selProject && (
                  <div className="row wrap">
                    <button className="btn sm" onClick={() => nav("projects", { id: selProject.id })}>Dashboard</button>
                    <button className="btn sm claude" onClick={() => launch(selProject, "claude")}>Claude</button>
                    <button className="btn sm codex" onClick={() => launch(selProject, "codex")}>Codex</button>
                    <button className="btn sm" onClick={() => nav("timeline", { project: selProject.id, ...(selected.kind === "stream" ? { workstream: selected.label } : {}) })}>Activity</button>
                    {selected.kind === "project" && <button className="btn sm" onClick={() => toggle(selected.id)}>{expanded.has(selected.id) ? "Collapse" : "Workstreams"}</button>}
                  </div>
                )}
              </div>
            </Card>
          )}
        </div>
      </div>
      <div className="legend">
        <span><i style={{ background: "var(--violet-glow)" }} />active &lt; 7 days</span>
        <span><i style={{ background: "var(--violet-soft)" }} />&lt; 30 days</span>
        <span><i style={{ background: "var(--dim)" }} />older</span>
        <span>node size = open NOW tasks · double-click a project for workstreams · drag to move · wheel to zoom</span>
      </div>
    </div>
  );
}
