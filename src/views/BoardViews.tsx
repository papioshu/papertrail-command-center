import { useEffect, useMemo, useState } from "react";
import { CaptureUpdateAction, convertToExcalidrawElements, exportToBlob } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { api, type Row } from "../api";

/** BOARDS B5 (D055): the board seen as a storyboard or a roadmap. Both are views over data that already exists:
 *  a storyboard shot is an Excalidraw frame whose details live on the frame (customData.shot), so it syncs with the
 *  board; the roadmap is the project's own hub tasks and milestones (no second task list), and any element on the
 *  board can be linked to a task (customData.task). */

type X = ExcalidrawImperativeAPI;
type El = { id: string; type: string; version: number; isDeleted?: boolean; name?: string | null; x: number; y: number; width: number; customData?: Row };
export type Shot = { scene?: string; duration?: number; angle?: string; move?: string; action?: string; dialogue?: string; status?: string; assignee?: string };

const ANGLES = ["", "Wide", "Medium", "Close-up", "Extreme close-up", "Over-the-shoulder", "POV", "Insert", "Bird's-eye", "Low angle"];
const MOVES = ["", "Static", "Pan", "Tilt", "Dolly", "Zoom", "Tracking", "Handheld", "Crane"];
const SHOT_STATUS = ["draft", "review", "approved"];
const TASK_STATUS = ["todo", "doing", "blocked", "done"];

/** Rewrites elements (new version, so every device takes it) and lets the board's own sync send them. */
export function patchEls(x: X, ids: Set<string>, f: (e: El) => Partial<El>) {
  const els = x.getSceneElementsIncludingDeleted().map((e) => (ids.has(e.id) ? { ...e, ...f(e as unknown as El), version: e.version + 1, versionNonce: (Math.random() * 2 ** 31) | 0, updated: Date.now() } : e));
  x.updateScene({ elements: els as never, captureUpdate: CaptureUpdateAction.IMMEDIATELY });
}

/** Shots in story order: by scene (1, 2, 10…), then left to right, top to bottom. */
export function storyOrder(frames: El[]): El[] {
  const scene = (f: El) => String(f.customData?.shot?.scene ?? "");
  return [...frames].sort((a, b) => scene(a).localeCompare(scene(b), undefined, { numeric: true }) || (Math.abs(a.y - b.y) > 40 ? a.y - b.y : a.x - b.x));
}

/** CSV of the shot list, quoted so commas and line breaks in dialogue survive. */
export function shotCsv(frames: El[], nameOf: (id: string) => string, taskTitle: (id: string) => string): string {
  const q = (v: unknown) => `"${String(v ?? "").replace(/"/g, '""')}"`;
  const rows = [["#", "Scene", "Shot", "Seconds", "Angle", "Movement", "Action", "Dialogue", "Status", "Assigned", "Task"]];
  storyOrder(frames).forEach((f, i) => {
    const s: Shot = f.customData?.shot ?? {};
    rows.push([String(i + 1), s.scene ?? "", f.name ?? "", String(s.duration ?? ""), s.angle ?? "", s.move ?? "", s.action ?? "", s.dialogue ?? "", s.status ?? "draft", s.assignee ? nameOf(s.assignee) : "", f.customData?.task ? taskTitle(f.customData.task) : ""]);
  });
  return rows.map((r) => r.map(q).join(",")).join("\r\n") + "\r\n";
}

// The board's frames and task-linked elements, re-read only when one of them changes.
function useScene(x: X) {
  const read = () => (x.getSceneElements() as unknown as El[]).filter((e) => e.type === "frame" || e.customData?.task);
  const [els, setEls] = useState(read);
  const [sel, setSel] = useState<string[]>([]);
  useEffect(() => {
    let sig = "", selSig = "";
    return x.onChange((all, st) => {
      const mine = (all as unknown as El[]).filter((e) => !e.isDeleted && (e.type === "frame" || e.customData?.task));
      const s = mine.map((e) => `${e.id}:${e.version}`).join();
      if (s !== sig) { sig = s; setEls(mine); }
      const ids = Object.keys(st.selectedElementIds ?? {}).filter((k) => st.selectedElementIds[k]);
      if (ids.join() !== selSig) { selSig = ids.join(); setSel(ids); }
    });
  }, [x]);
  return { frames: els.filter((e) => e.type === "frame"), linked: els.filter((e) => e.customData?.task), sel };
}

type Props = { x: X; ws: string; project: string; title: string; b: Row; canDraw: boolean; fail: (e: unknown) => void; reload: () => void; save: (name: string, blob: Blob) => Promise<void> };

export function Storyboard({ x, title, b, canDraw, fail, save }: Props) {
  const { frames } = useScene(x);
  const shots = storyOrder(frames);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [again, setAgain] = useState(0);
  const members: Row[] = b.members ?? [];
  const tasks: Row[] = (b.records ?? []).filter((r: Row) => r.kind === "task");
  const ids = shots.map((f) => f.id).join();
  // shortcut: thumbnails are drawn when the shot list changes or ↻ is pressed, not on every stroke; redraw per frame if boards get busy.
  useEffect(() => {
    let live = true;
    const made: Record<string, string> = {};
    (async () => {
      const all = x.getSceneElements();
      for (const f of frames) {
        const blob = await exportToBlob({ elements: all, appState: { ...x.getAppState(), exportBackground: true, exportWithDarkMode: false }, files: x.getFiles(), exportingFrame: f as never, maxWidthOrHeight: 320, mimeType: "image/png" }).catch(() => null);
        if (!live) return;
        if (blob) made[f.id] = URL.createObjectURL(blob);
      }
      setThumbs((old) => { Object.values(old).forEach(URL.revokeObjectURL); return made; });
    })();
    return () => { live = false; };
  }, [ids, again]); // eslint-disable-line react-hooks/exhaustive-deps

  const set = (f: El, k: keyof Shot, v: unknown) => {
    const shot = { ...(f.customData?.shot ?? {}), [k]: v === "" ? undefined : v };
    patchEls(x, new Set([f.id]), (e) => ({ customData: { ...(e.customData ?? {}), shot } }));
  };
  const addShot = () => {
    const st = x.getAppState();
    const last = shots[shots.length - 1];
    const at = last ? { x: Math.max(...frames.map((f) => f.x + f.width)) + 40, y: last.y } : { x: -st.scrollX + 80, y: -st.scrollY + 80 };
    const [el] = convertToExcalidrawElements([{ type: "frame", children: [], ...at, width: 480, height: 270, name: `Shot ${shots.length + 1}` } as never]);
    const withShot = { ...el, customData: { shot: { scene: last?.customData?.shot?.scene ?? "1", status: "draft" } } };
    x.updateScene({ elements: [...x.getSceneElementsIncludingDeleted(), withShot] as never, captureUpdate: CaptureUpdateAction.IMMEDIATELY });
    x.scrollToContent(withShot as never, { fitToViewport: true, viewportZoomFactor: 0.6, animate: true });
  };
  const exportList = () => {
    const nameOf = (id: string) => String(members.find((m) => m.user_id === id)?.name ?? "");
    const taskTitle = (id: string) => String(tasks.find((t) => t.id === id)?.fields?.title ?? "");
    const stamp = new Date().toISOString().slice(0, 16).replace("T", " ").replace(":", "");
    save(`${title} shot list ${stamp}.csv`, new Blob([shotCsv(frames, nameOf, taskTitle)], { type: "text/csv" })).catch(fail);
  };
  const total = shots.reduce((n, f) => n + (Number(f.customData?.shot?.duration) || 0), 0);
  const field = (f: El, k: keyof Shot) => String(f.customData?.shot?.[k] ?? "");

  return (
    <div className="card board-side" aria-label="Storyboard">
      <div className="row" style={{ gap: 6, alignItems: "center" }}>
        <b className="board-title" style={{ flex: 1 }}>Storyboard</b>
        <span className="dim small">{shots.length} shot{shots.length === 1 ? "" : "s"}{total ? ` · ${Math.floor(total / 60)}:${String(Math.round(total % 60)).padStart(2, "0")}` : ""}</span>
        <button className="btn sm ghost" title="Redraw thumbnails" aria-label="Redraw thumbnails" onClick={() => setAgain(again + 1)}>↻</button>
      </div>
      <div className="row" style={{ gap: 6 }}>
        {canDraw && <button className="btn sm primary" onClick={addShot}>+ Shot</button>}
        <button className="btn sm" disabled={!shots.length} onClick={exportList} title="Saves a CSV shot list to MEDIA › Boards">Export shot list</button>
      </div>
      {!shots.length && <div className="dim small">Each shot is a frame on the board. Add one with + Shot (or draw a frame, F), then fill in its scene, timing, camera and notes here.</div>}
      {shots.map((f, i) => (
        <div key={`${f.id}:${f.version}`} className="shot">
          <button className="shot-thumb" onClick={() => x.scrollToContent(f as never, { fitToViewport: true, viewportZoomFactor: 0.8, animate: true })} title="Show on the board">
            {thumbs[f.id] ? <img src={thumbs[f.id]} alt="" /> : <span className="dim small">#{i + 1}</span>}
          </button>
          <div className="col" style={{ gap: 4, flex: 1, minWidth: 0 }}>
            <div className="row" style={{ gap: 4 }}>
              <span className="chip">#{i + 1}</span>
              <input className="input sm" style={{ flex: 1 }} aria-label="Shot name" defaultValue={f.name ?? ""} disabled={!canDraw} maxLength={80}
                onBlur={(e) => e.target.value !== (f.name ?? "") && patchEls(x, new Set([f.id]), () => ({ name: e.target.value }))} />
              <select className="input sm" aria-label="Status" value={field(f, "status") || "draft"} disabled={!canDraw} onChange={(e) => set(f, "status", e.target.value)}>
                {SHOT_STATUS.map((s) => <option key={s}>{s}</option>)}
              </select>
            </div>
            <div className="row" style={{ gap: 4 }}>
              <input className="input sm" style={{ width: 70 }} aria-label="Scene" placeholder="Scene" defaultValue={field(f, "scene")} disabled={!canDraw} maxLength={20} onBlur={(e) => e.target.value !== field(f, "scene") && set(f, "scene", e.target.value)} />
              <input className="input sm" style={{ width: 64 }} aria-label="Seconds" placeholder="Sec" type="number" min={0} step={0.5} defaultValue={field(f, "duration")} disabled={!canDraw}
                onBlur={(e) => e.target.value !== field(f, "duration") && set(f, "duration", e.target.value === "" ? "" : Math.max(0, Number(e.target.value)))} />
            </div>
            <div className="row" style={{ gap: 4 }}>
              <select className="input sm" style={{ flex: 1 }} aria-label="Camera angle" value={field(f, "angle")} disabled={!canDraw} onChange={(e) => set(f, "angle", e.target.value)}>{ANGLES.map((a) => <option key={a} value={a}>{a || "Angle"}</option>)}</select>
              <select className="input sm" style={{ flex: 1 }} aria-label="Camera movement" value={field(f, "move")} disabled={!canDraw} onChange={(e) => set(f, "move", e.target.value)}>{MOVES.map((a) => <option key={a} value={a}>{a || "Move"}</option>)}</select>
            </div>
            <textarea className="input sm" rows={2} aria-label="Action" placeholder="Action" defaultValue={field(f, "action")} disabled={!canDraw} maxLength={1000} onBlur={(e) => e.target.value !== field(f, "action") && set(f, "action", e.target.value)} />
            <textarea className="input sm" rows={2} aria-label="Dialogue" placeholder="Dialogue" defaultValue={field(f, "dialogue")} disabled={!canDraw} maxLength={1000} onBlur={(e) => e.target.value !== field(f, "dialogue") && set(f, "dialogue", e.target.value)} />
            <div className="row" style={{ gap: 4 }}>
              <select className="input sm" style={{ flex: 1 }} aria-label="Assigned to" value={field(f, "assignee")} disabled={!canDraw} onChange={(e) => set(f, "assignee", e.target.value)}>
                <option value="">Unassigned</option>{members.map((m) => <option key={m.user_id} value={m.user_id}>{m.name}</option>)}
              </select>
              <select className="input sm" style={{ flex: 1 }} aria-label="Task" value={String(f.customData?.task ?? "")} disabled={!canDraw}
                onChange={(e) => { const task = e.target.value; patchEls(x, new Set([f.id]), (el) => { const { task: _old, ...rest } = el.customData ?? {}; return { customData: task ? { ...rest, task } : rest }; }); }}>
                <option value="">No task</option>{tasks.map((t) => <option key={t.id} value={t.id}>{t.fields?.title}</option>)}
              </select>
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}

export function Roadmap({ x, ws, project, b, canDraw, fail, reload }: Props) {
  const { linked, sel } = useScene(x);
  const recs: Row[] = (b.records ?? []).filter((r: Row) => r.project_id === project);
  const milestones = recs.filter((r) => r.kind === "milestone").sort((a, c) => String(a.fields?.due ?? "9").localeCompare(String(c.fields?.due ?? "9")));
  const tasks = recs.filter((r) => r.kind === "task");
  const nameOf = (id: string) => String((b.members ?? []).find((m: Row) => m.user_id === id)?.name ?? "");
  const [newTask, setNewTask] = useState("");
  const [into, setInto] = useState("");
  const [newMs, setNewMs] = useState("");
  // Elements on the board that point at each task: linked elements, plus shots whose task is set.
  const onBoard = useMemo(() => {
    const m = new Map<string, El[]>();
    for (const e of linked) m.set(String(e.customData?.task), [...(m.get(String(e.customData?.task)) ?? []), e]);
    return m;
  }, [linked]);
  const edit = (action: "create" | "update", kind: "task" | "milestone", fields: Row, id?: string) =>
    api.hubEdit(ws, action, kind, project, fields, id).then(reload, fail);
  const link = (task: string) => patchEls(x, new Set(sel), (e) => ({ customData: { ...(e.customData ?? {}), task } }));
  const unlink = (task: string) => patchEls(x, new Set((onBoard.get(task) ?? []).map((e) => e.id)), (e) => {
    const { task: _drop, ...rest } = e.customData ?? {};
    return { customData: rest };
  });
  const groups: [Row | null, Row[]][] = [...milestones.map((m): [Row, Row[]] => [m, tasks.filter((t) => t.fields?.milestone === m.id)]), [null, tasks.filter((t) => !milestones.some((m) => m.id === t.fields?.milestone))]];

  return (
    <div className="card board-side" aria-label="Roadmap">
      <div className="row" style={{ gap: 6, alignItems: "center" }}>
        <b className="board-title" style={{ flex: 1 }}>Roadmap</b>
        <span className="dim small">{tasks.filter((t) => t.fields?.status === "done").length}/{tasks.length} done{tasks.some((t) => t.fields?.status === "blocked") ? ` · ${tasks.filter((t) => t.fields?.status === "blocked").length} blocked` : ""}</span>
      </div>
      <div className="dim small">This project's team tasks and milestones (the same ones as in the workspace). {sel.length ? `${sel.length} selected on the board: use Link to attach them to a task.` : "Select things on the board to link them to a task."}</div>
      {groups.map(([m, list]) => (m || list.length ? (
        <div key={m?.id ?? "none"} className="col" style={{ gap: 4 }}>
          <div className="row" style={{ gap: 6, alignItems: "baseline" }}>
            <b className="small" style={{ flex: 1 }}>{m ? String(m.fields?.title) : "No milestone"}</b>
            {m?.fields?.due && <span className="dim small">due {m.fields.due}</span>}
            {m && <span className={`chip ${m.fields?.status === "done" ? "ok" : ""}`}>{m.fields?.status === "done" ? "done" : `${list.length ? Math.round((100 * list.filter((t) => t.fields?.status === "done").length) / list.length) : Number(m.fields?.progress ?? 0)}%`}</span>}
          </div>
          {list.map((t) => {
            const els = onBoard.get(t.id) ?? [];
            return (
              <div key={t.id} className={`road-task ${t.fields?.status ?? ""}`}>
                <span className="small" style={{ flex: 1, minWidth: 0, overflowWrap: "anywhere" }}>{t.fields?.title}
                  <span className="dim">{t.fields?.assignee ? ` · ${nameOf(t.fields.assignee)}` : ""}{t.fields?.due ? ` · due ${t.fields.due}` : ""}</span></span>
                <select className="input sm" aria-label={`Status of ${t.fields?.title}`} value={String(t.fields?.status ?? "todo")} disabled={!canDraw} onChange={(e) => edit("update", "task", { status: e.target.value }, t.id)}>
                  {TASK_STATUS.map((s) => <option key={s}>{s}</option>)}
                </select>
                {els.length > 0 && <button className="btn sm ghost" title="Show on the board" onClick={() => x.scrollToContent(els as never, { fitToViewport: true, viewportZoomFactor: 0.8, animate: true })}>↗ {els.length}</button>}
                {canDraw && sel.length > 0 && <button className="btn sm" onClick={() => link(t.id)}>Link</button>}
                {canDraw && els.length > 0 && <button className="btn sm ghost" title="Unlink the board items from this task" onClick={() => unlink(t.id)}>Unlink</button>}
              </div>
            );
          })}
        </div>
      ) : null))}
      {canDraw && (
        <div className="col" style={{ gap: 4 }}>
          <div className="row" style={{ gap: 4 }}>
            <input className="input sm" style={{ flex: 1 }} placeholder="New task" aria-label="New task" value={newTask} maxLength={200} onChange={(e) => setNewTask(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && newTask.trim() && edit("create", "task", { title: newTask.trim(), status: "todo", ...(into ? { milestone: into } : {}) }).then(() => setNewTask(""))} />
            <select className="input sm" aria-label="Milestone for the new task" value={into} onChange={(e) => setInto(e.target.value)}>
              <option value="">No milestone</option>{milestones.map((m) => <option key={m.id} value={m.id}>{m.fields?.title}</option>)}
            </select>
            <button className="btn sm primary" disabled={!newTask.trim()} onClick={() => edit("create", "task", { title: newTask.trim(), status: "todo", ...(into ? { milestone: into } : {}) }).then(() => setNewTask(""))}>+ Task</button>
          </div>
          <div className="row" style={{ gap: 4 }}>
            <input className="input sm" style={{ flex: 1 }} placeholder="New milestone" aria-label="New milestone" value={newMs} maxLength={200} onChange={(e) => setNewMs(e.target.value)} />
            <button className="btn sm" disabled={!newMs.trim()} onClick={() => edit("create", "milestone", { title: newMs.trim(), status: "open" }).then(() => setNewMs(""))}>+ Milestone</button>
          </div>
        </div>
      )}
    </div>
  );
}
