import { useEffect, useMemo, useRef, useState } from "react";
import { api, on, type Row } from "../api";
import { ago, brand, Card, Empty, ErrBox, useAsync, useNav } from "../ui";

/* 3D Book of Secrets network, drawn as SVG with a perspective projection (≈100 nodes: no GPU scene needed).
   Hub at the centre, workspaces on an inner shell, projects on an outer shell around their workspace,
   workstreams (git folders) beyond their project when expanded. Dashed arcs = explicit cross-references.
   Orbit by dragging (with inertia), zoom with the wheel, click to fly to a node, double-click a project for its
   workstreams. A slow auto-orbit runs only while visible and focused; the loop stops whenever nothing moves. */
type V3 = [number, number, number];
type N = { id: string; kind: "hub" | "ws" | "project" | "stream"; label: string; p: V3; r: number; color: string; row?: Row; parent?: string; ws?: string };
type E = { a: string; b: string; kind: "tree" | "link"; n?: number; files?: string };

const R1 = 230, R2 = 430, R3 = 120;
const norm = (v: V3): V3 => { const l = Math.hypot(...v) || 1; return [v[0] / l, v[1] / l, v[2] / l]; };
const cross = (a: V3, b: V3): V3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const fib = (i: number, n: number): V3 => { const y = 1 - (2 * (i + 0.5)) / n, r = Math.sqrt(1 - y * y), t = i * 2.39996; return [r * Math.cos(t), y, r * Math.sin(t)]; };
const around = (d: V3, i: number, n: number, spread: number): V3 => {
  if (n <= 1) return d;
  const t1 = norm(cross(d, Math.abs(d[1]) < 0.9 ? [0, 1, 0] : [1, 0, 0])), t2 = cross(d, t1), a = (i / n) * Math.PI * 2;
  return norm([d[0] + (t1[0] * Math.cos(a) + t2[0] * Math.sin(a)) * spread, d[1] + (t1[1] * Math.cos(a) + t2[1] * Math.sin(a)) * spread, d[2] + (t1[2] * Math.cos(a) + t2[2] * Math.sin(a)) * spread]);
};
const scale = (v: V3, s: number): V3 => [v[0] * s, v[1] * s, v[2] * s];

function recentColor(days: number) {
  return days < 7 ? "#c4b5fd" : days < 30 ? "#a78bfa" : "#6b6390";
}

function build(projects: Row[], streams: Row[], links: Row[], expanded: Set<string>) {
  const nodes: N[] = [{ id: "hub", kind: "hub", label: "BOOK OF SECRETS", p: [0, 0, 0], r: 16, color: "#8b5cf6" }];
  const edges: E[] = [];
  const groups = new Map<string, Row[]>();
  for (const p of projects) groups.set(p.workspace, [...(groups.get(p.workspace) ?? []), p]);
  const order = [...groups.keys()].sort();
  order.forEach((ws, wi) => {
    const d = fib(wi, Math.max(2, order.length));
    const g = brand(ws).glow;
    const members = groups.get(ws)!;
    const multi = members.length > 1;
    if (multi) {
      nodes.push({ id: `ws:${ws}`, kind: "ws", label: ws, p: scale(d, R1), r: 9, color: g, ws });
      edges.push({ a: "hub", b: `ws:${ws}`, kind: "tree" });
    }
    members.forEach((p, i) => {
      const pd = around(d, i, members.length, 0.22 + 0.02 * members.length);
      const days = p.last_activity ? (Date.now() - Date.parse(p.last_activity.length === 10 ? p.last_activity + "T12:00:00" : p.last_activity)) / 864e5 : 999;
      const pos = scale(pd, multi ? R2 : (R1 + R2) / 2);
      nodes.push({ id: p.id, kind: "project", label: p.name, p: pos, r: 6 + Math.min(7, Math.sqrt(p.active || 0) * 2), color: recentColor(days), row: { ...p, days }, ws });
      edges.push({ a: multi ? `ws:${ws}` : "hub", b: p.id, kind: "tree" });
      if (expanded.has(p.id)) {
        const ss = streams.filter((s) => s.project_id === p.id).sort((a, b) => b.commits - a.commits).slice(0, 7);
        ss.forEach((s, k) => {
          const sd = around(pd, k, ss.length, 0.09);
          const id = `st:${p.id}:${s.workstream}`;
          nodes.push({ id, kind: "stream", label: s.workstream, p: scale(sd, (multi ? R2 : (R1 + R2) / 2) + R3), r: 3 + Math.min(5, Math.log2(1 + s.commits)), color: "#b8a4e3", parent: p.id, row: s, ws });
          edges.push({ a: p.id, b: id, kind: "tree" });
        });
      }
    });
  });
  const ids = new Set(nodes.map((n) => n.id));
  for (const l of links) if (ids.has(l.project_id) && ids.has(l.target_id)) edges.push({ a: l.project_id, b: l.target_id, kind: "link", n: l.n, files: l.files });
  return { nodes, edges };
}

export default function Graph() {
  const { nav, params, launch } = useNav();
  const g = useAsync(() => api.graph(), [], ["projects-updated"]);
  const plist = useAsync(() => api.projects(true), [], ["projects-updated"]);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(params.focus ? [params.focus] : []));
  const [sel, setSel] = useState<string | null>(params.focus ?? null);
  const [hover, setHover] = useState<string | null>(null);
  const [q, setQ] = useState("");
  const [wsFilter, setWsFilter] = useState("");
  const [unsorted, setUnsorted] = useState(false);
  const [focusMode, setFocusMode] = useState(!!params.focus);
  const [cam, setCam] = useState({ yaw: 0.5, pitch: -0.25, zoom: 1 });
  const camRef = useRef(cam);
  camRef.current = cam;
  const wrap = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 1000, h: 700 });
  const motion = useRef({ vy: 0, vp: 0, dragging: false, lastInput: 0, fly: null as null | { t0: number; from: typeof cam; to: { yaw: number; pitch: number } } });
  const [, tick] = useState(0);

  const projects: Row[] = useMemo(() => (g.data?.projects ?? []).filter((p: Row) => (unsorted || p.workspace !== "UNSORTED") && (!wsFilter || p.workspace === wsFilter)), [g.data, unsorted, wsFilter]);
  const { nodes, edges } = useMemo(() => build(projects, g.data?.workstreams ?? [], g.data?.links ?? [], expanded), [projects, g.data, expanded]);
  const byId = useMemo(() => new Map(nodes.map((n) => [n.id, n])), [nodes]);

  // size
  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Animation loop: auto-orbit + inertia + fly-to, 30 fps cap, only while something moves and the window is active.
  useEffect(() => {
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    let raf = 0, last = performance.now(), minimized = false;
    const offMin = on("window-minimized", (m: boolean) => { minimized = m; });
    const loop = (now: number) => {
      raf = requestAnimationFrame(loop);
      if (now - last < 33) return;
      const dt = Math.min(0.1, (now - last) / 1000);
      last = now;
      if (minimized || document.visibilityState !== "visible" || !document.hasFocus()) return;
      const m = motion.current, c = camRef.current;
      let { yaw, pitch } = c;
      let moved = false;
      if (m.fly) {
        const t = Math.min(1, (now - m.fly.t0) / 700), k = t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
        yaw = m.fly.from.yaw + (m.fly.to.yaw - m.fly.from.yaw) * k;
        pitch = m.fly.from.pitch + (m.fly.to.pitch - m.fly.from.pitch) * k;
        if (t >= 1) m.fly = null;
        moved = true;
      } else if (!m.dragging) {
        if (Math.abs(m.vy) > 1e-4 || Math.abs(m.vp) > 1e-4) {
          yaw += m.vy; pitch = Math.max(-1.3, Math.min(1.3, pitch + m.vp));
          m.vy *= 0.92; m.vp *= 0.92; moved = true;
        } else if (!reduced && now - m.lastInput > 4000) {
          yaw += dt * 0.06; moved = true; // slow idle orbit
        }
      }
      if (moved) setCam({ ...c, yaw, pitch });
    };
    raf = requestAnimationFrame(loop);
    return () => { cancelAnimationFrame(raf); offMin.then((f) => f()); };
  }, []);

  // wheel zoom over the graph only
  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const w = (e: WheelEvent) => { e.preventDefault(); motion.current.lastInput = performance.now(); setCam((c) => ({ ...c, zoom: Math.max(0.45, Math.min(2.6, c.zoom * (e.deltaY < 0 ? 1.1 : 1 / 1.1))) })); };
    el.addEventListener("wheel", w, { passive: false });
    return () => el.removeEventListener("wheel", w);
  }, []);

  // projection
  const F = 900, D = 1250;
  const cy = Math.cos(cam.yaw), sy = Math.sin(cam.yaw), cp = Math.cos(cam.pitch), sp = Math.sin(cam.pitch);
  const proj = (p: V3) => {
    const x1 = p[0] * cy + p[2] * sy, z1 = -p[0] * sy + p[2] * cy;
    const y2 = p[1] * cp - z1 * sp, z2 = p[1] * sp + z1 * cp;
    const s = (F / (D - z2)) * cam.zoom * Math.min(size.w, size.h) / 800;
    return { x: size.w / 2 + x1 * s, y: size.h / 2 - y2 * s, z: z2, s };
  };
  const P = new Map(nodes.map((n) => [n.id, proj(n.p)]));
  const depthAlpha = (z: number) => 0.3 + 0.7 * Math.max(0, Math.min(1, (z + R2 + R3) / (2 * (R2 + R3))));

  const neighbors = useMemo(() => {
    if (!sel || !focusMode) return null;
    const s = new Set([sel]);
    for (const e of edges) { if (e.a === sel) s.add(e.b); if (e.b === sel) s.add(e.a); }
    return s;
  }, [sel, focusMode, edges]);
  const ql = q.trim().toLowerCase();
  const match = (n: N) => !!ql && n.label.toLowerCase().includes(ql);

  const flyTo = (id: string) => {
    const n = byId.get(id);
    if (!n || n.kind === "hub") return;
    const d = norm(n.p);
    const yaw = Math.atan2(-d[0], d[2]);
    const z1 = -d[0] * Math.sin(yaw) + d[2] * Math.cos(yaw);
    const pitch = -Math.atan2(d[1], z1);
    let dy = yaw - cam.yaw;
    dy = ((dy + Math.PI) % (2 * Math.PI) + 2 * Math.PI) % (2 * Math.PI) - Math.PI;
    motion.current.fly = { t0: performance.now(), from: cam, to: { yaw: cam.yaw + dy, pitch: Math.max(-1.2, Math.min(1.2, pitch)) } };
    motion.current.lastInput = performance.now();
  };
  const select = (id: string) => { setSel(id); flyTo(id); };
  const toggle = (id: string) => { const s = new Set(expanded); s.has(id) ? s.delete(id) : s.add(id); setExpanded(s); };

  // pointer: drag = orbit with inertia
  const drag = useRef<{ x: number; y: number; t: number; moved: boolean } | null>(null);
  const onDown = (e: React.PointerEvent) => { (e.currentTarget as Element).setPointerCapture(e.pointerId); drag.current = { x: e.clientX, y: e.clientY, t: performance.now(), moved: false }; motion.current.dragging = true; motion.current.vy = motion.current.vp = 0; motion.current.fly = null; };
  const onMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.x, dyy = e.clientY - d.y;
    if (Math.abs(dx) + Math.abs(dyy) > 3) d.moved = true;
    if (!d.moved) return;
    const ky = dx * 0.006, kp = dyy * 0.005;
    motion.current.vy = ky; motion.current.vp = kp;
    motion.current.lastInput = performance.now();
    setCam((c) => ({ ...c, yaw: c.yaw + ky, pitch: Math.max(-1.3, Math.min(1.3, c.pitch + kp)) }));
    d.x = e.clientX; d.y = e.clientY;
  };
  const onUp = (e: React.PointerEvent) => {
    const d = drag.current;
    drag.current = null;
    motion.current.dragging = false;
    if (d && !d.moved) {
      // click on empty space clears the selection; nodes handle their own clicks
      if ((e.target as Element).tagName === "svg") setSel(null);
      motion.current.vy = motion.current.vp = 0;
    }
    tick((x) => x + 1);
  };

  const selected = sel ? byId.get(sel) : undefined;
  const selProject = selected?.kind === "project" ? plist.data?.find((p) => p.id === selected.id) : selected?.kind === "stream" ? plist.data?.find((p) => p.id === selected.parent) : undefined;
  const selLinks = selected ? edges.filter((e) => e.kind === "link" && (e.a === selected.id || e.b === selected.id)) : [];
  const workspaces = [...new Set((g.data?.projects ?? []).map((p: Row) => p.workspace))].sort() as string[];
  const drawNodes = [...nodes].sort((a, b) => P.get(a.id)!.z - P.get(b.id)!.z);
  const colors = [...new Set(nodes.map((n) => n.color))];
  const gid = (c: string) => "glow" + c.replace(/[^a-z0-9]/gi, "");

  if (g.error) return <ErrBox error={g.error} />;
  return (
    <div className="col" style={{ gap: 12 }}>
      <div className="page-head" style={{ marginBottom: 0 }}>
        <div>
          <h1>The Graph</h1>
          <div className="sub">Book of Secrets in 3D · drag to orbit · wheel to zoom · click to fly · double-click a project for its workstreams</div>
        </div>
        <div className="right">
          <input className="input" placeholder="Search nodes" value={q} onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") { const n = nodes.find(match); if (n) select(n.id); } }} style={{ width: 170 }} />
          <select className="input" value={wsFilter} onChange={(e) => setWsFilter(e.target.value)}>
            <option value="">All workspaces</option>
            {workspaces.map((w) => <option key={w} value={w}>{w}</option>)}
          </select>
          <label className="row small muted"><input type="checkbox" checked={unsorted} onChange={(e) => setUnsorted(e.target.checked)} /> unsorted</label>
          <label className="row small muted"><input type="checkbox" checked={focusMode} onChange={(e) => setFocusMode(e.target.checked)} /> focus selection</label>
          <button className="btn sm" onClick={() => setExpanded(new Set(projects.map((p) => p.id)))}>Expand all</button>
          <button className="btn sm" onClick={() => setExpanded(new Set())}>Collapse</button>
          <button className="btn sm" onClick={() => { setCam({ yaw: 0.5, pitch: -0.25, zoom: 1 }); motion.current.fly = null; }}>Reset</button>
        </div>
      </div>
      <div className="graph-wrap g3d" ref={wrap}>
        {!projects.length && !g.loading && <Empty>No projects to graph</Empty>}
        <svg onPointerDown={onDown} onPointerMove={onMove} onPointerUp={onUp} onPointerCancel={onUp}>
          <defs>
            {colors.map((c) => (
              <radialGradient key={c} id={gid(c)}>
                <stop offset="0%" stopColor="#ffffff" stopOpacity="0.95" />
                <stop offset="22%" stopColor={c} stopOpacity="0.95" />
                <stop offset="55%" stopColor={c} stopOpacity="0.28" />
                <stop offset="100%" stopColor={c} stopOpacity="0" />
              </radialGradient>
            ))}
          </defs>
          {/* depth grid rings around the hub */}
          {[R1, R2].map((r) => {
            const pts = Array.from({ length: 64 }, (_, i) => { const a = (i / 64) * Math.PI * 2; const q2 = proj([Math.cos(a) * r, 0, Math.sin(a) * r]); return `${q2.x},${q2.y}`; }).join(" ");
            return <polygon key={r} points={pts} fill="none" stroke="var(--line)" strokeOpacity={0.6} strokeDasharray="3 6" />;
          })}
          {edges.map((e, i) => {
            const a = P.get(e.a), b = P.get(e.b);
            if (!a || !b) return null;
            const dim = neighbors && !(neighbors.has(e.a) && neighbors.has(e.b));
            const alpha = (depthAlpha((a.z + b.z) / 2)) * (dim ? 0.15 : 1);
            if (e.kind === "link") {
              const mx = (a.x + b.x) / 2 + (size.w / 2 - (a.x + b.x) / 2) * 0.35, my = (a.y + b.y) / 2 + (size.h / 2 - (a.y + b.y) / 2) * 0.35;
              return <path key={i} d={`M${a.x},${a.y} Q${mx},${my} ${b.x},${b.y}`} fill="none" stroke="#a78bfa" strokeOpacity={0.75 * alpha} strokeDasharray="5 5" strokeWidth={Math.min(3, 1 + Math.log2(1 + (e.n ?? 1)) / 2)} />;
            }
            return <line key={i} x1={a.x} y1={a.y} x2={b.x} y2={b.y} stroke="#3a2f68" strokeOpacity={0.9 * alpha} strokeWidth={e.a === "hub" ? 1.4 : 1} />;
          })}
          {drawNodes.map((n) => {
            const s = P.get(n.id)!;
            const dim = neighbors && !neighbors.has(n.id);
            const hot = sel === n.id || hover === n.id || match(n);
            const a = depthAlpha(s.z) * (dim ? 0.2 : 1);
            const r = n.r * s.s * (hot ? 1.35 : 1);
            const showLabel = n.kind !== "stream" || expanded.has(n.parent ?? "") ;
            return (
              <g key={n.id} className="node" opacity={a}
                onPointerDown={(e) => e.stopPropagation()}
                onClick={() => (n.kind === "ws" ? (setSel(n.id), flyTo(n.id)) : select(n.id))}
                onDoubleClick={() => n.kind === "project" ? toggle(n.id) : n.kind === "ws" ? nav("workspace", { ws: n.ws }) : null}
                onMouseEnter={() => setHover(n.id)} onMouseLeave={() => setHover(null)}>
                <circle cx={s.x} cy={s.y} r={r * 3.2} fill={`url(#${gid(n.color)})`} />
                {n.kind === "hub" ? <rect x={s.x - r * 1.6} y={s.y - r} width={r * 3.2} height={r * 2} fill="rgba(22,16,43,.9)" stroke="#8b5cf6" strokeWidth={1.5} rx={3} />
                  : n.kind === "ws" ? <rect x={s.x - r} y={s.y - r} width={r * 2} height={r * 2} transform={`rotate(45 ${s.x} ${s.y})`} fill="rgba(11,8,20,.85)" stroke={n.color} strokeWidth={hot ? 2.2 : 1.4} />
                  : <circle cx={s.x} cy={s.y} r={r} fill={n.kind === "stream" ? "rgba(11,8,20,.9)" : n.color} stroke={hot ? "#fff" : n.kind === "stream" ? "#b8a4e3" : "rgba(255,255,255,.35)"} strokeWidth={hot ? 1.8 : 0.8} />}
                {n.kind === "hub" ? <text x={s.x} y={s.y + 3.5} textAnchor="middle" style={{ fontSize: 9.5, letterSpacing: "0.2em" }}>BOOK OF SECRETS</text>
                  : showLabel && <text x={s.x + r + 6} y={s.y + 3.5} style={{ fontSize: (n.kind === "ws" ? 11 : n.kind === "stream" ? 9 : 10) * Math.max(0.8, Math.min(1.25, s.s * 1.1)), fill: n.kind === "ws" ? n.color : n.kind === "stream" ? "var(--muted)" : undefined }}>
                    {n.kind === "ws" ? n.label.toUpperCase() : n.kind === "project" ? (n.label.match(/\((.+)\)/)?.[1] ?? n.label) : n.label}
                  </text>}
              </g>
            );
          })}
        </svg>
        <div className="graph-side">
          {selected && selected.kind !== "hub" && (
            <Card title={selected.kind === "ws" ? "Workspace" : selected.kind === "stream" ? "Workstream" : "Project"}>
              <div className="col">
                <div className="display" style={{ fontSize: 15 }}>{selected.label}</div>
                {selected.kind === "ws" && (
                  <>
                    <div className="muted small">{nodes.filter((n) => n.kind === "project" && n.ws === selected.ws).length} projects</div>
                    <button className="btn sm primary" onClick={() => nav("workspace", { ws: selected.ws })}>Open workspace overview</button>
                  </>
                )}
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
                          return <div key={i} className="small" style={{ cursor: "pointer" }} title={e.files} onClick={() => other && select(other.id)}>↔ {other?.label} <span className="dim">({e.n}×)</span></div>;
                        })}
                      </div>
                    )}
                  </>
                )}
                {selProject && (
                  <div className="row wrap">
                    <button className="btn sm" onClick={() => nav("projects", { id: selProject.id })}>Dashboard</button>
                    <button className="btn sm" onClick={() => nav("workspace", { ws: selProject.workspace })}>Workspace</button>
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
        <span><i style={{ background: "#c4b5fd" }} />active &lt; 7 days</span>
        <span><i style={{ background: "#a78bfa" }} />&lt; 30 days</span>
        <span><i style={{ background: "#6b6390" }} />older</span>
        <span>◇ workspace (double-click opens its overview) · node size = open NOW tasks · nearer = larger and brighter</span>
      </div>
    </div>
  );
}
