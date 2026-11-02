import { useEffect, useMemo, useRef, useState } from "react";
import { on, type Row } from "../api";
import { brand, parentOf } from "../ui";
import { faceAngles, layoutNodes, liveEngines, qualityTier, SphereEngine, type SphereLink, type SphereNode } from "./engine";

/* Interactive Book of Secrets sphere. Modes:
   - "gpu": animated WebGL2 (30 fps cap, 12 fps when idle, paused when hidden/unfocused/off-screen)
   - "static": WebGL works but is software-rendered (or reduced motion): frames only on interaction
   - "svg": no WebGL at all: same node layout as an SVG, same hover/click navigation */
type Mode = "gpu" | "static" | "svg";

const hex = (h: string): [number, number, number] => {
  const m = h.replace("#", "");
  const n = parseInt(m.length === 3 ? m.split("").map((c) => c + c).join("") : m, 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
};
const daysSince = (ts?: string | null) => (ts ? Math.max(0, (Date.now() - Date.parse(ts.length === 10 ? ts + "T12:00:00" : ts)) / 864e5) : 999);

export default function Sphere({ projects, links, selected, highlightWorkspace, onHover, onOpen }: {
  projects: Row[]; links: Row[]; selected: string | null; highlightWorkspace?: string | null; onHover: (id: string | null) => void; onOpen: (id: string) => void;
}) {
  const canvas = useRef<HTMLCanvasElement | null>(null);
  const wrap = useRef<HTMLDivElement>(null);
  const engine = useRef<SphereEngine | null>(null);
  const [mode, setMode] = useState<Mode>("gpu");
  const [labels, setLabels] = useState<{ id: string; x: number; y: number; z: number }[]>([]);
  const [hover, setHover] = useState<string | null>(null);
  const transitioning = useRef(false);

  const sig = projects.map((p) => `${p.id}:${p.workspace}:${Math.floor(daysSince(p.last_activity))}:${p.active_tasks}`).join("|") + "#" + links.map((l) => `${l.project_id}>${l.target_id}`).join(",");
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const parents = useMemo(() => parentOf(projects), [sig]);
  const nodes: SphereNode[] = useMemo(() => projects.map((p) => {
    const g = hex(brand(p.workspace).glow);
    // brand identity, pulled toward the sphere's lavender so the palette stays coherent
    return { id: p.id, workspace: p.workspace, color: [g[0] * 0.6 + 0.3, g[1] * 0.6 + 0.26, g[2] * 0.6 + 0.4], activityDays: daysSince(p.last_activity), weight: p.active_tasks || 0, parent: parents.get(p.id) };
  }), [sig]);
  const sLinks: SphereLink[] = useMemo(() => {
    const ids = new Set(projects.map((p) => p.id));
    const seen = new Set<string>();
    return links.filter((l) => ids.has(l.project_id) && ids.has(l.target_id)).flatMap((l) => {
      const k = [l.project_id, l.target_id].sort().join("|");
      if (seen.has(k)) return [];
      seen.add(k);
      return [{ a: l.project_id, b: l.target_id, n: l.n }];
    });
  }, [sig]);
  const byId = useMemo(() => new Map(projects.map((p) => [p.id, p])), [projects]);

  // Build the engine after first paint so it never delays the dashboard.
  useEffect(() => {
    if (!nodes.length) return;
    let disposed = false;
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const idle = (window as any).requestIdleCallback ?? ((f: () => void) => setTimeout(f, 50));
    const el = document.createElement("canvas");
    wrap.current?.prepend(el);
    canvas.current = el;
    const handle = idle(() => {
      if (disposed || !canvas.current) return;
      try {
        const e = new SphereEngine(canvas.current, nodes, sLinks, qualityTier());
        const renderer = e.rendererInfo();
        const software = /swiftshader|basic render|llvmpipe|software/i.test(renderer);
        e.reduced = reduced || software;
        let lastLabels = 0;
        // Labels track nodes at ~15 Hz: the revolve moves them < 1 px per frame, so this halves React work.
        e.onFrame = () => { const now = performance.now(); if (now - lastLabels > 60 || !e.running) { lastLabels = now; setLabels(e.project()); } };
        engine.current = e;
        const m: Mode = software || reduced ? "static" : "gpu";
        setMode(m);
        (window as any).__sphereStats = () => ({ mode: m, renderer, particles: e.particleCount, fps: e.fps, running: e.running, engines: liveEngines() });
        e.draw();
        if (m === "gpu" && document.visibilityState === "visible" && document.hasFocus()) e.start();
      } catch (err) {
        console.warn("sphere: falling back to SVG", err);
        setMode("svg");
      }
    });
    return () => {
      disposed = true;
      (window as any).cancelIdleCallback?.(handle);
      engine.current?.dispose();
      engine.current = null;
      el.remove(); // a disposed (context-lost) canvas can never get WebGL again: each build owns a fresh one
      canvas.current = null;
    };
  }, [nodes, sLinks]);

  // Pause when hidden, unfocused or scrolled out of view; idle → 12 fps; activity → 30 fps.
  useEffect(() => {
    let visible = true;
    let active = true; // native signal: false while minimized (WebView2 still reports visible + focused then)
    let idleTimer = 0;
    const sync = () => {
      const e = engine.current;
      if (!e || mode !== "gpu") return;
      const run = active && visible && document.visibilityState === "visible" && document.hasFocus() && !transitioning.current;
      if (run) e.start(); else e.stop();
    };
    const wake = () => {
      const e = engine.current;
      if (!e) return;
      e.fps = 24;
      clearTimeout(idleTimer);
      idleTimer = window.setTimeout(() => { if (engine.current) engine.current.fps = 10; }, 45000);
    };
    const io = new IntersectionObserver(([en]) => { visible = en.isIntersecting; sync(); });
    const offActive = on("window-minimized", (m: boolean) => { active = !m; sync(); });
    if (wrap.current) io.observe(wrap.current);
    document.addEventListener("visibilitychange", sync);
    window.addEventListener("focus", sync);
    window.addEventListener("blur", sync);
    window.addEventListener("pointermove", wake);
    wake();
    sync();
    return () => {
      io.disconnect();
      offActive.then((f) => f());
      document.removeEventListener("visibilitychange", sync);
      window.removeEventListener("focus", sync);
      window.removeEventListener("blur", sync);
      window.removeEventListener("pointermove", wake);
      clearTimeout(idleTimer);
    };
  }, [mode]);

  // Keep the selected node highlighted.
  useEffect(() => {
    const e = engine.current;
    if (!e) return;
    e.focus = selected ? e.index(selected) : -1;
    if (!e.running) e.draw();
  }, [selected, mode]);

  // Workspace card hover lights that workspace's nodes on the map.
  useEffect(() => {
    const e = engine.current;
    if (!e) return;
    e.wsHover = e.wsIdx(highlightWorkspace ?? null);
    if (!e.running) e.draw();
  }, [highlightWorkspace, mode]);

  // Wheel over the map zooms the map only; elsewhere the page scrolls as usual (needs a non-passive listener).
  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const onWheelNative = (ev: WheelEvent) => {
      const e = engine.current;
      if (!e) return;
      ev.preventDefault();
      e.dist = Math.max(2.3, Math.min(4.6, e.dist + ev.deltaY * 0.0015));
      if (!e.running) e.draw();
    };
    el.addEventListener("wheel", onWheelNative, { passive: false });
    return () => el.removeEventListener("wheel", onWheelNative);
  }, [mode]);

  /* --- pointer: hover pick, drag to rotate, click to open --- */
  const drag = useRef<{ x: number; y: number; yaw: number; tilt: number; moved: boolean } | null>(null);
  const pick = (cx: number, cy: number) => {
    const r = wrap.current!.getBoundingClientRect();
    const x = cx - r.left, y = cy - r.top;
    let best: string | null = null, bd = 22;
    for (const l of labels) {
      if (l.z < -0.25) continue; // back hemisphere is not clickable
      const d = Math.hypot(l.x - x, l.y - y);
      if (d < bd) { bd = d; best = l.id; }
    }
    return best;
  };
  const setHoverId = (id: string | null) => {
    if (id === hover) return;
    setHover(id);
    onHover(id);
    const e = engine.current;
    if (e) {
      e.hover = id ? e.index(id) : -1;
      e.hoverAt = performance.now();
      if (!e.running) e.draw();
    }
  };
  const onDown = (ev: React.PointerEvent) => {
    const e = engine.current;
    drag.current = { x: ev.clientX, y: ev.clientY, yaw: e?.yaw ?? 0, tilt: e?.tilt ?? 0, moved: false };
  };
  const onMove = (ev: React.PointerEvent) => {
    const d = drag.current, e = engine.current;
    if (d && e && (ev.buttons & 1)) {
      const dx = ev.clientX - d.x, dy = ev.clientY - d.y;
      if (Math.abs(dx) + Math.abs(dy) > 4) d.moved = true;
      if (d.moved) {
        e.yaw = d.yaw + dx * 0.006;
        e.tilt = Math.max(-1.2, Math.min(1.2, d.tilt + dy * 0.004));
        if (!e.running) e.draw();
        return;
      }
    }
    setHoverId(pick(ev.clientX, ev.clientY));
  };
  const onUp = (ev: React.PointerEvent) => {
    const d = drag.current;
    drag.current = null;
    if (d?.moved) return;
    const id = pick(ev.clientX, ev.clientY);
    if (id) open(id);
  };

  /** Rotate the node to the front and zoom in, then hand over to the project dashboard. */
  function open(id: string) {
    const e = engine.current;
    const dir = e?.dir(id);
    if (!e || !dir || transitioning.current || mode === "svg") return onOpen(id);
    transitioning.current = true;
    e.stop();
    const target = faceAngles(dir);
    const from = { yaw: e.yaw, tilt: e.tilt, dist: e.dist };
    let dy = target.yaw - from.yaw;
    dy = ((dy + Math.PI) % (2 * Math.PI) + 2 * Math.PI) % (2 * Math.PI) - Math.PI; // shortest way round
    const t0 = performance.now(), dur = window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 1 : 650;
    const step = (now: number) => {
      const t = Math.min(1, (now - t0) / dur), k = t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
      e.yaw = from.yaw + dy * k;
      e.tilt = from.tilt + (target.tilt - from.tilt) * k;
      e.dist = from.dist + (1.9 - from.dist) * k;
      e.draw();
      if (t < 1) requestAnimationFrame(step);
      else { transitioning.current = false; onOpen(id); }
    };
    requestAnimationFrame(step);
  }

  // Labels: hovered, selected and the three most recently active front-facing nodes. Restrained on purpose.
  const recent = useMemo(() => [...projects].sort((a, b) => String(b.last_activity ?? "").localeCompare(String(a.last_activity ?? ""))).slice(0, 3).map((p) => p.id), [projects]);
  void recent;
  // Every project is named, but labels never pile up: place by priority (hover/selected, lit workspace,
  // front-most) and drop any lower-priority label whose box would overlap one already placed.
  const shortName = (n?: string) => (n ? n.match(/\((.+)\)/)?.[1] ?? n : "");
  const shown = useMemo(() => {
    const pri = (l: { id: string; z: number }) => {
      const p = byId.get(l.id);
      return (l.id === hover || l.id === selected ? 10 : 0) + (highlightWorkspace && p?.workspace === highlightWorkspace ? 5 : 0) + l.z;
    };
    const placed: { x0: number; y0: number; x1: number; y1: number }[] = [];
    const out: typeof labels = [];
    for (const l of [...labels].sort((a, b) => pri(b) - pri(a))) {
      if (parents.has(l.id) && l.id !== hover && l.id !== selected) continue; // satellites are named on hover
      const w = shortName(byId.get(l.id)?.name).length * 7.4 + 18, box = { x0: l.x + 10, y0: l.y - 10, x1: l.x + 10 + w, y1: l.y + 6 };
      const hit = placed.some((b) => box.x0 < b.x1 && box.x1 > b.x0 && box.y0 < b.y1 && box.y1 > b.y0);
      if (hit && pri(l) < 10) continue;
      placed.push(box);
      out.push(l);
    }
    return out;
  }, [labels, hover, selected, highlightWorkspace, byId, parents]);
  const kidCount = useMemo(() => { const m = new Map<string, number>(); for (const v of parents.values()) m.set(v, (m.get(v) ?? 0) + 1); return m; }, [parents]);

  if (mode === "svg") return <SvgFallback projects={projects} links={sLinks} selected={selected} onHover={onHover} onOpen={onOpen} />;
  return (
    <div ref={wrap} className="sphere" onPointerDown={onDown} onPointerMove={onMove} onPointerUp={onUp} onPointerLeave={() => setHoverId(null)}
      style={{ cursor: hover ? "pointer" : "grab" }}>
      {shown.map((l) => {
        const p = byId.get(l.id);
        const hot = l.id === hover || l.id === selected || (!!highlightWorkspace && p?.workspace === highlightWorkspace);
        return (
          <div key={l.id} className={`slabel ${hot ? "hot" : ""}`} style={{ transform: `translate(${l.x + 14}px, ${l.y - 8}px)`, opacity: hot ? 1 : l.z > 0 ? 0.6 + 0.4 * l.z : 0.32 + 0.25 * (1 + l.z), zIndex: hot ? 3 : l.z > 0 ? 2 : 1 }}>
            <span className="n">{shortName(p?.name)}{kidCount.get(l.id) ? <span className="plus"> +{kidCount.get(l.id)}</span> : null}</span>
            {(l.id === hover || l.id === selected) && shortName(p?.name) !== p?.workspace && <span className="m">{p?.workspace}</span>}
          </div>
        );
      })}
      {mode === "static" && <div className="smode label">static render · motion off</div>}
    </div>
  );
}

/** No-WebGL fallback: identical layout, orthographic, interactive. */
function SvgFallback({ projects, links, selected, onHover, onOpen }: { projects: Row[]; links: SphereLink[]; selected: string | null; onHover: (id: string | null) => void; onOpen: (id: string) => void }) {
  const nodes: SphereNode[] = projects.map((p) => ({ id: p.id, workspace: p.workspace, color: [0.7, 0.6, 1], activityDays: daysSince(p.last_activity), weight: p.active_tasks || 0 }));
  const dirs = layoutNodes(nodes);
  const yaw = 0.6, tilt = 0.32;
  const proj = (d: [number, number, number]) => {
    const x1 = d[0] * Math.cos(yaw) + d[2] * Math.sin(yaw), z1 = -d[0] * Math.sin(yaw) + d[2] * Math.cos(yaw);
    const y2 = d[1] * Math.cos(tilt) - z1 * Math.sin(tilt), z2 = d[1] * Math.sin(tilt) + z1 * Math.cos(tilt);
    return { x: 200 + x1 * 170, y: 200 - y2 * 170, z: z2 };
  };
  return (
    <div className="sphere">
      <svg viewBox="0 0 400 400" style={{ width: "100%", height: "100%" }}>
        <defs><radialGradient id="sg"><stop offset="60%" stopColor="#1b2a8a" stopOpacity="0.15" /><stop offset="100%" stopColor="#4d6bff" stopOpacity="0.5" /></radialGradient></defs>
        <circle cx={200} cy={200} r={170} fill="url(#sg)" stroke="#5b7bff" strokeOpacity={0.5} />
        {links.map((l, i) => { const a = dirs.get(l.a), b = dirs.get(l.b); if (!a || !b) return null; const p = proj(a), q = proj(b); return <line key={i} x1={p.x} y1={p.y} x2={q.x} y2={q.y} stroke="#b8a4e3" strokeOpacity={0.25} />; })}
        {projects.map((p) => { const d = dirs.get(p.id)!; const s = proj(d); const front = s.z > -0.2; return (
          <g key={p.id} opacity={front ? 1 : 0.3} style={{ cursor: "pointer" }} onMouseEnter={() => onHover(p.id)} onMouseLeave={() => onHover(null)} onClick={() => onOpen(p.id)}>
            <circle cx={s.x} cy={s.y} r={p.id === selected ? 7 : 5} fill={brand(p.workspace).glow} />
            {front && s.z > 0.15 && <text x={s.x + 9} y={s.y + 3} fill="#ece8fb" fontSize={8} fontFamily="Consolas, monospace">{p.name}</text>}
          </g>
        ); })}
      </svg>
      <div className="smode label">WebGL unavailable · static map</div>
    </div>
  );
}
