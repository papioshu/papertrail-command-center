import { Component, createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, on, type LaunchReq, type Row } from "./api";
import cardBombsquad from "./assets/card-bombsquad.webp";
import cardCaedis from "./assets/card-caedis.webp";
import cardPapershoots from "./assets/card-papershoots.webp";
import cardZathena from "./assets/card-zathena.webp";

/** PapertrailOS profile art + accents (DESIGN_SYSTEM.md per-profile accent). Other workspaces get a drawn card. */
const BRANDS: [RegExp, string | null, string][] = [
  [/BOMBSQUAD|BMBSQD/i, cardBombsquad, "#aef044"],
  [/ZATHENA|^ZV$/i, cardZathena, "#c9a227"],
  [/PAPERSHOOT/i, cardPapershoots, "#b8a4e3"],
  [/CAEDIS/i, cardCaedis, "#5b8def"],
];
const DRAWN = ["#8b5cf6", "#a78bfa", "#b8a4e3", "#5b8def", "#d8cfc0"];
export function brand(workspace: string): { img: string | null; glow: string } {
  const b = BRANDS.find(([re]) => re.test(workspace));
  if (b) return { img: b[1], glow: b[2] };
  let h = 0;
  for (const c of workspace) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return { img: null, glow: DRAWN[h % DRAWN.length] };
}

/* ---------- navigation ---------- */
export type View = "overview" | "workspace" | "projects" | "actions" | "agents" | "launchpad" | "graph" | "discovery" | "timeline" | "settings";
export type NavFn = (v: View, params?: Row) => void;
export const NavCtx = createContext<{ nav: NavFn; params: Row; launch: (p: Row, agent?: "claude" | "codex", mode?: LaunchReq["mode"], preset?: string, task?: string) => void }>({
  nav: () => {},
  params: {},
  launch: () => {},
});
export const useNav = () => useContext(NavCtx);

/* ---------- data ---------- */
export function useAsync<T>(fn: () => Promise<T>, deps: unknown[], refreshOn: string[] = []) {
  const [state, set] = useState<{ data?: T; error?: string; loading: boolean }>({ loading: true });
  const seq = useRef(0);
  const load = useCallback(() => {
    const n = ++seq.current;
    set((s) => ({ ...s, loading: true }));
    fn().then(
      (data) => n === seq.current && set({ data, loading: false }),
      (e) => n === seq.current && set((s) => ({ ...s, error: String(e), loading: false })),
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  useEffect(() => {
    load();
    const offs = refreshOn.map((ev) => on(ev, () => load()));
    return () => offs.forEach((p) => p.then((f) => f()));
  }, [load]);
  return { ...state, reload: load };
}

/* ---------- formatting ---------- */
export function ago(ts?: string | null): string {
  if (!ts) return "—";
  const t = Date.parse(ts.length === 10 ? ts + "T12:00:00" : ts);
  if (isNaN(t)) return ts;
  const s = (Date.now() - t) / 1000;
  if (s < 0) return "today";
  if (s < 90) return "just now";
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400 * 1.5) return `${Math.round(s / 3600)}h ago`;
  if (s < 86400 * 60) return `${Math.round(s / 86400)}d ago`;
  return new Date(t).toISOString().slice(0, 10);
}
export const day = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
export const short = (s?: string | null, n = 8) => (s ? s.slice(0, n) : "—");

/** Live = transcript written in the last 3 minutes (evidence, not a process check). */
export function sessionState(updated?: string | null): { label: string; cls: string } {
  if (!updated) return { label: "unknown", cls: "" };
  const s = (Date.now() - Date.parse(updated)) / 1000;
  if (s < 180) return { label: "writing now", cls: "live" };
  if (s < 3600) return { label: "recently active", cls: "active" };
  return { label: "idle", cls: "" };
}

/* ---------- small pieces ---------- */
export function Barcode({ value, height = 22, width = 64 }: { value: string; height?: number; width?: number }) {
  // Deterministic bars from the string (an identity mark for the record, like a ticket serial).
  const bars = useMemo(() => {
    let h = 2166136261;
    const out: number[] = [];
    for (let i = 0; i < 28; i++) {
      h ^= value.charCodeAt(i % Math.max(1, value.length)) + i;
      h = Math.imul(h, 16777619) >>> 0;
      out.push(1 + (h % 3));
    }
    return out;
  }, [value]);
  let x = 0;
  const total = bars.reduce((a, b) => a + b + 1, 0);
  return (
    <svg className="barcode" width={width} height={height} viewBox={`0 0 ${total} 10`} preserveAspectRatio="none" aria-hidden>
      {bars.map((w, i) => {
        const r = i % 2 === 0 ? <rect key={i} x={x} y={0} width={w} height={10} fill="currentColor" /> : null;
        x += w + 1;
        return r;
      })}
    </svg>
  );
}

export const Empty = ({ children }: { children: ReactNode }) => <div className="empty">{children}</div>;
export const ErrBox = ({ error }: { error: string }) => <div className="errbox">{error}</div>;
export const AgentChip = ({ agent }: { agent?: string | null }) =>
  agent === "claude" || agent === "codex" ? <span className={`chip ${agent}`}>{agent}</span> : <span className="chip">{agent || "unattributed"}</span>;

export function Card({ title, right, children, className = "", body = true }: { title: ReactNode; right?: ReactNode; children: ReactNode; className?: string; body?: boolean }) {
  return (
    <section className={`card ${className}`}>
      <div className="card-head">
        <span className="label">{title}</span>
        {right && <div className="right">{right}</div>}
      </div>
      {body ? <div className="card-body">{children}</div> : children}
    </section>
  );
}

export class Boundary extends Component<{ name: string; children: ReactNode }, { error?: string }> {
  state: { error?: string } = {};
  static getDerivedStateFromError(e: unknown) {
    return { error: String(e) };
  }
  render() {
    if (this.state.error)
      return (
        <div className="errbox">
          {this.props.name} failed to render: {this.state.error}{" "}
          <button className="btn sm" onClick={() => this.setState({ error: undefined })}>Retry</button>
        </div>
      );
    return this.props.children;
  }
}

export function Modal({ title, onClose, children, foot }: { title: ReactNode; onClose: () => void; children: ReactNode; foot?: ReactNode }) {
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-modal="true">
        <div className="card-head">
          <span className="label">{title}</span>
          <div className="right"><button className="btn sm ghost" onClick={onClose} aria-label="Close">Close</button></div>
        </div>
        <div className="card-body">{children}</div>
        {foot && <div className="modal-foot">{foot}</div>}
      </div>
    </div>
  );
}

/* ---------- time range ---------- */
export type Range = { key: string; from: string; to: string };
export function rangeFor(key: string, custom?: { from: string; to: string }): Range {
  const to = new Date();
  const days: Record<string, number> = { today: 0, "7d": 6, "30d": 29, "90d": 89, all: 3650 };
  if (key === "custom" && custom) return { key, ...custom };
  const from = new Date(to);
  from.setDate(from.getDate() - (days[key] ?? 29));
  return { key, from: day(from), to: day(to) };
}
export function RangePicker({ value, onChange }: { value: Range; onChange: (r: Range) => void }) {
  const keys = ["today", "7d", "30d", "90d", "all", "custom"];
  return (
    <div className="row">
      <div className="seg">
        {keys.map((k) => (
          <button key={k} className={value.key === k ? "on" : ""} onClick={() => onChange(rangeFor(k, { from: value.from, to: value.to }))}>
            {k.toUpperCase()}
          </button>
        ))}
      </div>
      {value.key === "custom" && (
        <>
          <input className="input mono" type="date" value={value.from} onChange={(e) => onChange({ ...value, from: e.target.value })} />
          <input className="input mono" type="date" value={value.to} onChange={(e) => onChange({ ...value, to: e.target.value })} />
        </>
      )}
    </div>
  );
}

/* ---------- stacked day chart ---------- */
export type Series = { key: string; label: string; color: string };
export function DayChart({ rows, series, height = 170, onPick, from, to }: { rows: Row[]; series: Series[]; height?: number; onPick?: (day: string) => void; from: string; to: string }) {
  const [tip, setTip] = useState<{ x: number; y: number; r: Row } | null>(null);
  // Fill every day in range so gaps read as zero, not as missing time. Long ranges bucket by week.
  const filled = useMemo(() => {
    const by = new Map(rows.map((r) => [r.day, r]));
    const start = new Date(from + "T00:00:00");
    const end = new Date(to + "T00:00:00");
    const first = rows.length ? new Date(rows[0].day + "T00:00:00") : start;
    const s = start < first && (end.getTime() - start.getTime()) / 864e5 > 400 ? first : start;
    const out: Row[] = [];
    for (let d = new Date(s); d <= end; d.setDate(d.getDate() + 1)) out.push(by.get(day(d)) ?? { day: day(d) });
    if (out.length <= 120) return out;
    const weeks: Row[] = [];
    for (let i = 0; i < out.length; i += 7) {
      const chunk = out.slice(i, i + 7);
      const w: Row = { day: chunk[0].day, until: chunk[chunk.length - 1].day };
      for (const sr of series) w[sr.key] = chunk.reduce((a, r) => a + (r[sr.key] || 0), 0);
      weeks.push(w);
    }
    return weeks;
  }, [rows, series, from, to]);
  const W = 900;
  const pad = { l: 34, r: 8, t: 8, b: 20 };
  const max = Math.max(1, ...filled.map((r) => series.reduce((a, s) => a + (r[s.key] || 0), 0)));
  const bw = (W - pad.l - pad.r) / Math.max(1, filled.length);
  const y = (v: number) => pad.t + (height - pad.t - pad.b) * (1 - v / max);
  const ticks = [...new Set([0, Math.round(max / 2), max])];
  const labelEvery = Math.ceil(filled.length / 10);
  return (
    <div>
      <svg className="chart" viewBox={`0 0 ${W} ${height}`} onMouseLeave={() => setTip(null)}>
        {ticks.map((t) => (
          <g key={t}>
            <line className="axis" x1={pad.l} x2={W - pad.r} y1={y(t)} y2={y(t)} strokeDasharray={t ? "2 4" : undefined} />
            <text x={pad.l - 6} y={y(t) + 3} textAnchor="end">{t}</text>
          </g>
        ))}
        {filled.map((r, i) => {
          let acc = 0;
          const x = pad.l + i * bw;
          return (
            <g key={r.day} onMouseMove={(e) => setTip({ x: e.clientX, y: e.clientY, r })} onClick={() => onPick?.(r.day)} style={{ cursor: onPick ? "pointer" : undefined }}>
              <rect x={x} y={pad.t} width={bw} height={height - pad.t - pad.b} fill="transparent" />
              {series.map((s) => {
                const v = r[s.key] || 0;
                if (!v) return null;
                const y0 = y(acc + v);
                const h = y(acc) - y0;
                acc += v;
                return <rect key={s.key} x={x + 1} y={y0} width={Math.max(1, bw - 2)} height={h} fill={s.color} />;
              })}
              {i % labelEvery === 0 && <text x={x + bw / 2} y={height - 6} textAnchor="middle">{r.day.slice(5)}</text>}
            </g>
          );
        })}
      </svg>
      <div className="legend">
        {series.map((s) => (
          <span key={s.key}><i style={{ background: s.color }} />{s.label}</span>
        ))}
      </div>
      {tip && (
        <div className="tip" style={{ left: tip.x + 12, top: tip.y + 12 }}>
          <div className="label">{tip.r.day}{tip.r.until ? ` → ${tip.r.until}` : ""}</div>
          {series.map((s) => (
            <div key={s.key}><span style={{ color: s.color }}>■</span> {s.label}: {tip.r[s.key] || 0}</div>
          ))}
          {onPick && <div className="dim small">click for evidence</div>}
        </div>
      )}
    </div>
  );
}

/* ---------- virtualized sortable table ---------- */
export type Col = { key: string; label: string; width?: number | string; render?: (r: Row) => ReactNode; sort?: (r: Row) => any };
export function DataTable({ rows, cols, height = 360, onRow, filter = true, initialSort, selected }: { rows: Row[]; cols: Col[]; height?: number; onRow?: (r: Row) => void; filter?: boolean; initialSort?: [string, 1 | -1]; selected?: (r: Row) => boolean }) {
  const [q, setQ] = useState("");
  const [sort, setSort] = useState<[string, 1 | -1] | undefined>(initialSort);
  const [top, setTop] = useState(0);
  const RH = 30;
  const view = useMemo(() => {
    const ql = q.trim().toLowerCase();
    let v = ql ? rows.filter((r) => cols.some((c) => String(r[c.key] ?? "").toLowerCase().includes(ql))) : rows.slice();
    if (sort) {
      const c = cols.find((c) => c.key === sort[0]);
      const get = c?.sort ?? ((r: Row) => r[sort[0]]);
      v.sort((a, b) => {
        const x = get(a), y = get(b);
        return (x === y ? 0 : x == null ? 1 : y == null ? -1 : x > y ? 1 : -1) * sort[1];
      });
    }
    return v;
  }, [rows, cols, q, sort]);
  const start = Math.max(0, Math.floor(top / RH) - 10);
  const end = Math.min(view.length, start + Math.ceil(height / RH) + 20);
  return (
    <div>
      {filter && (
        <div className="row" style={{ padding: "8px 14px" }}>
          <input className="input" placeholder="Filter…" value={q} onChange={(e) => setQ(e.target.value)} style={{ width: 240 }} />
          <span className="dim small mono">{view.length} of {rows.length}</span>
        </div>
      )}
      <div className="table-wrap" style={{ maxHeight: height }} onScroll={(e) => setTop(e.currentTarget.scrollTop)}>
        <table className="t">
          <colgroup>{cols.map((c) => <col key={c.key} style={{ width: c.width }} />)}</colgroup>
          <thead>
            <tr>
              {cols.map((c) => (
                <th key={c.key} onClick={() => setSort(sort?.[0] === c.key ? [c.key, sort[1] === 1 ? -1 : 1] : [c.key, -1])}>
                  {c.label}{sort?.[0] === c.key ? (sort[1] === 1 ? " ▲" : " ▼") : ""}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {start > 0 && <tr style={{ height: start * RH }} />}
            {view.slice(start, end).map((r, i) => (
              <tr key={start + i} className={`${onRow ? "click" : ""} ${selected?.(r) ? "sel" : ""}`} onClick={() => onRow?.(r)}>
                {cols.map((c) => (
                  <td key={c.key} title={typeof r[c.key] === "string" ? r[c.key] : undefined}>{c.render ? c.render(r) : r[c.key] ?? "—"}</td>
                ))}
              </tr>
            ))}
            {end < view.length && <tr style={{ height: (view.length - end) * RH }} />}
          </tbody>
        </table>
        {!view.length && <Empty>No records</Empty>}
      </div>
    </div>
  );
}

/* ---------- launch dialog ---------- */
const MODES: { key: LaunchReq["mode"]; label: string; hint: string }[] = [
  { key: "continue", label: "Continue last session", hint: "Reopen the most recent session for this project with the CLI's own resume." },
  { key: "fresh", label: "Fresh handoff", hint: "New session that reads a generated brief: state, open tasks, commits, files to read." },
  { key: "targeted", label: "Targeted task", hint: "Fresh handoff aimed at a preset task or workstream." },
  { key: "custom", label: "Custom prompt", hint: "Your own task, combined with the project brief." },
];

export function LaunchDialog({ project, agent: a0 = "claude", mode: m0 = "fresh", preset: p0, task: t0 = "", onClose }: { project: Row; agent?: "claude" | "codex"; mode?: LaunchReq["mode"]; preset?: string; task?: string; onClose: () => void }) {
  const [agent, setAgent] = useState<"claude" | "codex">(a0);
  const [mode, setMode] = useState<LaunchReq["mode"]>(p0 ? "targeted" : m0);
  const [preset, setPreset] = useState<string | undefined>(p0);
  const [task, setTask] = useState(t0);
  const [preview, setPreview] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<Row | null>(null);
  const [error, setError] = useState<string | null>(null);
  const settings = useAsync(() => api.settings(), []);
  const presets: Row[] = (settings.data?.presets ?? []).filter(
    (p: Row) => (!p.project || p.project === project.id) && (p.agent === "any" || p.agent === agent),
  );
  const last = agent === "claude" ? project.last_claude : project.last_codex;
  const lastS = typeof last === "string" ? JSON.parse(last) : last;
  const req = (force = false): LaunchReq => ({
    project_id: project.id, agent, mode,
    task: mode === "custom" || mode === "targeted" ? task : undefined,
    preset_id: mode === "targeted" ? preset : undefined, force,
  });
  const can = mode === "continue" ? !!lastS : mode === "custom" ? task.trim().length > 0 : mode === "targeted" ? !!preset : true;
  async function go(force = false) {
    setBusy(true); setError(null);
    try {
      const r = await api.launch(req(force));
      setResult(r);
    } catch (e) { setError(String(e)); }
    setBusy(false);
  }
  async function doPreview() {
    setError(null);
    try { setPreview(await api.previewHandoff(req())); } catch (e) { setError(String(e)); }
  }
  return (
    <Modal title={<>Launch · {project.name}</>} onClose={onClose}
      foot={<>
        {result?.status === "launched" && <span className="chip ok">Launched in {String(result.cwd)}</span>}
        {mode !== "continue" && <button className="btn" onClick={doPreview} disabled={!can}>Preview brief</button>}
        <button className={`btn primary`} disabled={!can || busy || result?.status === "launched"} onClick={() => go(false)}>
          {busy ? "Launching…" : `Launch ${agent}`}
        </button>
      </>}>
      <div className="col">
        <div className="row">
          <span className="label">Agent</span>
          <div className="seg">
            <button className={agent === "claude" ? "on" : ""} onClick={() => { setAgent("claude"); setPreset(undefined); }}>CLAUDE CODE</button>
            <button className={agent === "codex" ? "on" : ""} onClick={() => { setAgent("codex"); setPreset(undefined); }}>CODEX</button>
          </div>
          <span className="dim small mono" style={{ marginLeft: "auto" }}>{project.repo_path || project.vault_dir}</span>
        </div>
        <div className="grid g4">
          {MODES.map((m) => (
            <button key={m.key} className={`card pad ${mode === m.key ? "tt-on" : ""}`} style={{ textAlign: "left", cursor: "pointer", borderColor: mode === m.key ? "var(--violet)" : undefined }}
              onClick={() => { setMode(m.key); setPreview(null); setResult(null); }}>
              <div className="display" style={{ fontSize: 13 }}>{m.label}</div>
              <div className="muted small" style={{ marginTop: 4 }}>{m.hint}</div>
            </button>
          ))}
        </div>
        {mode === "continue" && (
          lastS ? <div className="notice">Resumes {agent} session <span className="mono">{short(lastS.id, 12)}</span>, “{lastS.title}”, last written {ago(lastS.updated)}{lastS.cwd ? <> in <span className="mono">{lastS.cwd}</span></> : null}.</div>
                : <div className="notice">No recorded {agent} session is mapped to this project. Use Fresh handoff.</div>
        )}
        {mode === "targeted" && (
          <div className="row wrap">
            {presets.map((p) => (
              <button key={p.id} className={`btn ${preset === p.id ? "primary" : ""}`} onClick={() => setPreset(p.id)} title={p.task}>{p.name}</button>
            ))}
            {!presets.length && <span className="muted small">No presets for this agent. Add them in Settings.</span>}
          </div>
        )}
        {(mode === "custom" || mode === "targeted") && (
          <textarea className="input" rows={4} placeholder={mode === "custom" ? "Describe the task…" : "Optional note added to the preset task"} value={task} onChange={(e) => setTask(e.target.value)} />
        )}
        {error && <ErrBox error={error} />}
        {result?.status === "conflict" && (
          <div className="errbox">{String(result.message)} <button className="btn sm danger" onClick={() => go(true)}>Launch anyway</button></div>
        )}
        {result?.status === "duplicate" && <div className="notice">{String(result.message)}</div>}
        {result?.status === "launched" && result.handoff && (
          <div className="notice">Brief: <a href="#" onClick={(e) => { e.preventDefault(); api.openPath(String(result.handoff)); }}>{String(result.handoff)}</a></div>
        )}
        {preview && <pre className="doc">{preview}</pre>}
      </div>
    </Modal>
  );
}

/** Quick-action row for a project. Every button does something real. */
export function ProjectActions({ p, compact = false }: { p: Row; compact?: boolean }) {
  const { nav, launch } = useNav();
  const dir = p.repo_path || p.vault_dir;
  const cls = compact ? "btn sm" : "btn";
  return (
    <div className="actions row wrap">
      <button className={`${cls} claude`} onClick={() => launch(p, "claude")}>Claude</button>
      <button className={`${cls} codex`} onClick={() => launch(p, "codex")}>Codex</button>
      <button className={cls} onClick={() => launch(p, p.last_claude && (!p.last_codex || JSON.parse(p.last_claude).updated >= JSON.parse(p.last_codex).updated) ? "claude" : "codex", "continue")}
        disabled={!p.last_claude && !p.last_codex}>Continue</button>
      <button className={cls} onClick={() => dir && api.openPath(dir)} disabled={!dir}>Open</button>
      <button className={cls} onClick={() => nav("timeline", { project: p.id })}>Changes</button>
      {p.vault_dir && <button className={cls} onClick={() => api.openPath(p.vault_dir)}>Logs</button>}
      <button className={cls} onClick={() => nav("graph", { focus: p.id })}>Graph</button>
    </div>
  );
}

/* ---------- drag-to-scroll (horizontal decks) ---------- */
/** Click-and-drag scrolls the element; a real drag swallows the click that ends it. */
export function useDragScroll<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let down = false, moved = false, sx = 0, sl = 0;
    const onDown = (e: PointerEvent) => { if (e.button !== 0) return; down = true; moved = false; sx = e.clientX; sl = el.scrollLeft; };
    const onMove = (e: PointerEvent) => {
      if (!down) return;
      const dx = e.clientX - sx;
      if (!moved && Math.abs(dx) > 5) { moved = true; el.setPointerCapture(e.pointerId); el.style.cursor = "grabbing"; }
      if (moved) el.scrollLeft = sl - dx;
    };
    const onUp = () => { down = false; el.style.cursor = ""; };
    const onClick = (e: MouseEvent) => { if (moved) { e.stopPropagation(); e.preventDefault(); moved = false; } };
    // Vertical wheel over the deck scrolls it sideways.
    const onWheel = (e: WheelEvent) => { if (Math.abs(e.deltaY) > Math.abs(e.deltaX) && el.scrollWidth > el.clientWidth) { el.scrollLeft += e.deltaY; e.preventDefault(); } };
    el.addEventListener("pointerdown", onDown);
    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerup", onUp);
    el.addEventListener("pointercancel", onUp);
    el.addEventListener("click", onClick, true);
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      el.removeEventListener("pointerdown", onDown); el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerup", onUp); el.removeEventListener("pointercancel", onUp);
      el.removeEventListener("click", onClick, true); el.removeEventListener("wheel", onWheel);
    };
  }, []);
  return ref;
}

/* ---------- status bar + sparkline (on the work itself) ---------- */
export const STATUS_ORDER = ["done", "active", "pending", "blocked", "deferred"] as const;
export const STATUS_LABEL: Record<string, string> = { done: "Done", active: "Active", pending: "Pending", blocked: "Blocked", deferred: "Deferred" };
export const STATUS_COLOR: Record<string, string> = { done: "var(--lime)", active: "var(--violet)", pending: "var(--violet-soft)", blocked: "var(--gold)", deferred: "var(--dim)" };

/** Segmented bar of parsed TASKS.md items by status. Counts only; no invented percentages. */
export function StatusBar({ counts, height = 6, legend = false }: { counts: Record<string, number>; height?: number; legend?: boolean }) {
  const total = STATUS_ORDER.reduce((a, s) => a + (counts[s] || 0), 0);
  if (!total) return <div className="dim small mono" style={{ fontSize: 10 }}>no TASKS.md items</div>;
  return (
    <div className="col" style={{ gap: 5 }}>
      <div style={{ display: "flex", height, borderRadius: 3, overflow: "hidden", background: "rgba(7,5,15,.6)" }}>
        {STATUS_ORDER.map((s) => counts[s] ? <div key={s} title={`${STATUS_LABEL[s]}: ${counts[s]}`} style={{ width: `${(100 * counts[s]) / total}%`, background: STATUS_COLOR[s] }} /> : null)}
      </div>
      {legend && (
        <div className="row wrap" style={{ gap: 10 }}>
          {STATUS_ORDER.filter((s) => counts[s]).map((s) => (
            <span key={s} className="mono" style={{ fontSize: 9.5, color: "var(--muted)", letterSpacing: ".08em" }}>
              <span style={{ display: "inline-block", width: 7, height: 7, borderRadius: 2, background: STATUS_COLOR[s], marginRight: 4 }} />{STATUS_LABEL[s].toUpperCase()} {counts[s]}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

/** Tiny bar sparkline of recorded events per day. */
export function Spark({ values, width = 150, height = 26, color = "var(--violet-glow)" }: { values: number[]; width?: number; height?: number; color?: string }) {
  const max = Math.max(1, ...values);
  const bw = width / Math.max(1, values.length);
  return (
    <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} aria-label={`${values.reduce((a, b) => a + b, 0)} events`}>
      <line x1={0} x2={width} y1={height - 0.5} y2={height - 0.5} stroke="var(--line)" />
      {values.map((v, i) => v ? <rect key={i} x={i * bw + 1} y={height - 1 - (v / max) * (height - 3)} width={Math.max(1, bw - 2)} height={(v / max) * (height - 3)} fill={color} rx={1}><title>{v}</title></rect> : null)}
    </svg>
  );
}

/* ---------- discovery ordering (radar explore) ---------- */
export type RadarOrder = "top" | "shuffle" | "mix";
/** top = by score; shuffle = seeded random; mix = round-robin across topics so one topic can't fill the row. */
export function orderDiscoveries(items: Row[], mode: RadarOrder, seed: number): Row[] {
  if (mode === "top") return items;
  if (mode === "shuffle") {
    let s = seed >>> 0 || 1;
    const rnd = () => ((s = (s * 1664525 + 1013904223) >>> 0) / 4294967296);
    const a = items.slice();
    for (let i = a.length - 1; i > 0; i--) { const j = Math.floor(rnd() * (i + 1)); [a[i], a[j]] = [a[j], a[i]]; }
    return a;
  }
  const topic = (x: Row) => String(x.reason ?? "").match(/topic "([^"]+)"/)?.[1] ?? x.source ?? "other";
  const groups = new Map<string, Row[]>();
  for (const x of items) groups.set(topic(x), [...(groups.get(topic(x)) ?? []), x]);
  const lists = [...groups.values()];
  const out: Row[] = [];
  for (let i = 0; out.length < items.length; i++) for (const l of lists) if (l[i]) out.push(l[i]);
  return out;
}
export function RadarOrderControl({ mode, setMode, reshuffle }: { mode: RadarOrder; setMode: (m: RadarOrder) => void; reshuffle: () => void }) {
  return (
    <div className="seg" title="Top = by relevance score · Shuffle = random explore · Mix = one of each topic in turn">
      {(["top", "shuffle", "mix"] as RadarOrder[]).map((m) => (
        <button key={m} className={mode === m ? "on" : ""} onClick={() => { if (m === "shuffle" && mode === "shuffle") reshuffle(); setMode(m); if (m === "shuffle") reshuffle(); }}>
          {m === "shuffle" && mode === "shuffle" ? "↻ SHUFFLE" : m.toUpperCase()}
        </button>
      ))}
    </div>
  );
}

/** Sub-project → parent project: a project named "<Parent> …" or "<Parent> (…)" in the same workspace sits under it
 *  (BOMBSQUAD Bounty Board → BOMBSQUAD). Longest matching parent wins. */
export function parentOf(projects: Row[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const p of projects) {
    const best = projects
      .filter((q) => q.id !== p.id && q.workspace === p.workspace && (p.name.startsWith(q.name + " ") || p.name.startsWith(q.name + "(")))
      .sort((a, b) => b.name.length - a.name.length)[0];
    if (best) out.set(p.id, best.id);
  }
  return out;
}
