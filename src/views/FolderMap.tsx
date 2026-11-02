import { useMemo, useState } from "react";
import { api, type Row } from "../api";
import { ago, Empty, Modal, useAsync } from "../ui";

/** Every real folder in the project (repo + vault, two levels) with what was recorded in it.
 *  Heat = lines changed in commits; Claude edit calls and recency shown alongside. Untouched folders stay listed. */
export default function FolderMap({ projectId, files }: { projectId: string; files: Row[] }) {
  const folders = useAsync(() => api.folders(projectId), [projectId], ["projects-updated"]);
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [kind, setKind] = useState<"repo" | "vault">("repo");
  const [pick, setPick] = useState<Row | null>(null);
  const all = folders.data ?? [];
  const kinds = useMemo(() => [...new Set(all.map((f) => f.kind))] as ("repo" | "vault")[], [all]);
  const k = kinds.includes(kind) ? kind : kinds[0] ?? "repo";
  const list = all.filter((f) => f.kind === k);
  const max = Math.max(1, ...list.map((f) => f.changes + f.edits * 20));
  const worked = list.filter((f) => f.depth === 0 && (f.commits || f.edits)).length;
  const top = list.filter((f) => f.depth === 0).sort((a, b) => (b.last_ts ?? "").localeCompare(a.last_ts ?? "") || a.path.localeCompare(b.path));
  const heat = (f: Row) => {
    const d = f.last_ts ? (Date.now() - Date.parse(f.last_ts)) / 864e5 : 999;
    return d < 2 ? "var(--violet-glow)" : d < 14 ? "var(--violet)" : d < 60 ? "var(--violet-soft)" : "var(--dim)";
  };
  const row = (f: Row) => {
    const kids = list.filter((c) => c.depth === f.depth + 1 && c.path.startsWith(f.path + "/"));
    const isOpen = open.has(f.path);
    const w = ((f.changes + f.edits * 20) / max) * 100;
    return (
      <div key={f.path}>
        <div className="frow" style={{ paddingLeft: 6 + f.depth * 18, opacity: f.exists ? 1 : 0.55 }} onClick={() => setPick(f)}>
          <span className="tw" onClick={(e) => { e.stopPropagation(); if (!kids.length) return; const s = new Set(open); isOpen ? s.delete(f.path) : s.add(f.path); setOpen(s); }}>
            {kids.length ? (isOpen ? "▾" : "▸") : "·"}
          </span>
          <span className="mono ellipsis" title={f.path}>{f.path.split("/").pop()}{!f.exists && <span className="dim"> (deleted)</span>}</span>
          <div className="track"><div className="fill" style={{ width: `${Math.max(f.commits || f.edits ? 2 : 0, w)}%`, background: heat(f) }} /></div>
          <span className="mono small dim" title="commits touching this folder">{f.commits || "—"}</span>
          <span className="mono small" style={{ color: "var(--claude)" }} title="Claude edit tool calls">{f.edits || ""}</span>
          <span className="mono small dim">{f.last_ts ? ago(f.last_ts) : "untouched"}</span>
        </div>
        {isOpen && kids.sort((a, b) => (b.last_ts ?? "").localeCompare(a.last_ts ?? "")).map(row)}
      </div>
    );
  };
  const pickFiles = pick ? files.filter((x) => pick.path === "(root files)" ? !x.path.includes("/") : x.path.startsWith(pick.path + "/")).slice(0, 300) : [];
  return (
    <section className="card">
      <div className="card-head">
        <span className="label">Folders · where the work happened</span>
        <div className="right">
          <span className="dim small">{worked} of {list.filter((f) => f.depth === 0).length} top-level folders worked on</span>
          {kinds.length > 1 && <div className="seg">{kinds.map((x) => <button key={x} className={k === x ? "on" : ""} onClick={() => setKind(x)}>{x.toUpperCase()}</button>)}</div>}
        </div>
      </div>
      <div className="card-body" style={{ maxHeight: 420, overflow: "auto", paddingTop: 6 }}>
        <div className="frow head"><span /><span>folder</span><span>lines changed + edits</span><span>commits</span><span>claude</span><span>last</span></div>
        {top.map(row)}
        {!list.length && <Empty>{folders.loading ? "Reading folders…" : "No folders found"}</Empty>}
      </div>
      {pick && (
        <Modal title={`${k} · ${pick.path}`} onClose={() => setPick(null)} foot={<span className="dim small">{pick.commits} commits · {pick.changes} lines changed · {pick.edits} Claude edit calls · last {pick.last_ts ? ago(pick.last_ts) : "never"}</span>}>
          {k === "repo" && pickFiles.length ? (
            <div className="list">{pickFiles.map((x) => (
              <div key={x.path} className="li"><span className="chip">{x.commits}×</span><span className="mono small ellipsis">{x.path}</span><span className="dim small">{ago(x.last_ts)}</span></div>
            ))}</div>
          ) : <Empty>{k === "vault" ? "Vault folder: activity is the newest note's modification time" : "No committed files recorded in this folder"}</Empty>}
        </Modal>
      )}
    </section>
  );
}
