import { lazy, Suspense, useState } from "react";
import { api, type Row } from "../api";
import { ago, Empty } from "../ui";

/** Workspace BOARDS (D044): the board library of one workspace. A board belongs to one of its projects (that's
 *  where its access, media and sync come from). The canvas engine loads only when a board is opened. */

const BoardCanvas = lazy(() => import("./BoardCanvas"));

const RANK: Record<string, number> = { owner: 0, admin: 1, editor: 2, contributor: 3, viewer: 4 };
const can = (role: string, need: string) => (RANK[role] ?? 9) <= RANK[need];
const BG = { grid: "Dark grid", blackboard: "Blackboard" } as const;

export default function Boards({ ws, b, project, me, onErr, reload }: { ws: string; b: Row; project: string; me: string; onErr: (e: string) => void; reload: () => void }) {
  const boards: Row[] = (b.records ?? []).filter((r: Row) => r.kind === "board" && (!project || r.project_id === project));
  const projects: Row[] = b.projects ?? [];
  const [openId, setOpenId] = useState("");
  const [name, setName] = useState("");
  const [bg, setBg] = useState<keyof typeof BG>("grid");
  const [target, setTarget] = useState("");
  const [renaming, setRenaming] = useState<{ id: string; title: string } | null>(null);
  const [deleting, setDeleting] = useState("");
  const into = project || target || projects[0]?.id || "";
  const fail = (e: unknown) => onErr(String(e));
  const editor = can(b.role, "contributor");
  const opened = boards.find((x) => x.id === openId) ?? (b.records ?? []).find((r: Row) => r.id === openId);

  if (opened) {
    return (
      <Suspense fallback={<div className="board-shell" />}>
        <BoardCanvas ws={ws} board={opened} b={b} canDraw={editor} onBack={() => setOpenId("")} onErr={onErr} reload={reload} />
      </Suspense>
    );
  }
  // A workspace with no project yet gets one named "Boards" for its boards (owners/admins only can add projects).
  const create = async () => {
    if (!name.trim()) return;
    try {
      const pid = into || String((await api.hubNewProject(ws, "Boards")).id);
      const id = await api.hubEdit(ws, "create", "board", pid, { title: name.trim(), bg });
      setName(""); reload(); setOpenId(id);
    } catch (e) { fail(e); }
  };
  const mine = (r: Row) => r.created_by === me || can(b.role, "editor");
  return (
    <div className="col" style={{ gap: 12 }}>
      {editor && (into || can(b.role, "admin")) && (
        <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
          <input className="input" placeholder="Board name, e.g. FLY WING moodboard" value={name} aria-label="Board name"
            onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && create()} style={{ minWidth: 260 }} />
          <select className="input" value={bg} onChange={(e) => setBg(e.target.value as keyof typeof BG)} aria-label="Background">
            {Object.entries(BG).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
          </select>
          {!project && projects.length > 1 && (
            <select className="input" value={into} onChange={(e) => setTarget(e.target.value)} aria-label="Project">
              {projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
            </select>
          )}
          <button className="btn sm primary" disabled={!name.trim()} onClick={create}>New board</button>
        </div>
      )}
      {!boards.length ? <Empty>{editor && (into || can(b.role, "admin")) ? "No boards yet. Name one above to start a canvas." : editor ? "No boards yet. An admin needs to add a project to this workspace first." : "No boards here yet."}</Empty> : (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(240px, 1fr))", gap: 12 }}>
          {boards.map((r) => (
            <div key={r.id} className={`card board-card ${r.fields?.bg === "blackboard" ? "blackboard" : "grid"}`}>
              {renaming && renaming.id === r.id ? (
                <input className="input" autoFocus value={renaming.title} aria-label="New board name"
                  onChange={(e) => setRenaming({ id: r.id, title: e.target.value })}
                  onKeyDown={(e) => {
                    if (e.key === "Escape") setRenaming(null);
                    if (e.key === "Enter" && renaming.title.trim()) api.hubEdit(ws, "update", "board", r.project_id, { title: renaming.title.trim() }, r.id).then(() => { setRenaming(null); reload(); }, fail);
                  }} />
              ) : (
                <button className="board-open" onClick={() => setOpenId(r.id)} title="Open board">
                  <span className="board-title">{r.fields?.title}</span>
                </button>
              )}
              <div className="dim small">{projects.find((p) => p.id === r.project_id)?.name} · {BG[r.fields?.bg as keyof typeof BG] ?? "Dark grid"} · {r.pending ? "waiting to sync" : ago(r.updated_at)}</div>
              {editor && mine(r) && (
                <div className="row" style={{ gap: 6 }}>
                  <button className="btn sm ghost" onClick={() => setRenaming({ id: r.id, title: String(r.fields?.title ?? "") })}>Rename</button>
                  {deleting === r.id ? (
                    <>
                      <button className="btn sm" style={{ color: "var(--signal)" }} onClick={() => api.hubEdit(ws, "delete", "board", r.project_id, null, r.id).then(() => { setDeleting(""); reload(); }, fail)}>Delete for everyone</button>
                      <button className="btn sm ghost" onClick={() => setDeleting("")}>Keep</button>
                    </>
                  ) : <button className="btn sm ghost" onClick={() => setDeleting(r.id)}>Delete</button>}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
