import { useState } from "react";
import { api, type Row } from "../api";
import { ago, Empty, ErrBox, Modal, useAsync } from "../ui";

/** FILES (D048): a shared project's files. Teammates download them and propose changes or new files; the owner's PC
 *  (the one that shared the project) previews each proposal and accepts it into the repo or declines it. */

const RANK: Record<string, number> = { owner: 0, admin: 1, editor: 2, contributor: 3, viewer: 4 };
const can = (role: string, need: string) => (RANK[role] ?? 9) <= RANK[need];
const fmt = (n: number) => (n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`);
const readData = (f: File) => new Promise<string>((ok, no) => { const r = new FileReader(); r.onload = () => ok(String(r.result)); r.onerror = () => no(new Error("Couldn't read that file.")); r.readAsDataURL(f); });
const leaf = (p: string) => p.slice(p.lastIndexOf("/") + 1);

async function download(ws: string, id: string, path: string) {
  const a = document.createElement("a");
  a.href = String(await api.hubFile(ws, id, leaf(path)));
  a.download = leaf(path);
  a.click();
}

function Review({ ws, project, p, onClose, onDone }: { ws: string; project: string; p: Row; onClose: () => void; onDone: () => void }) {
  const pv = useAsync(() => api.hubProposalPreview(ws, project, p.id), [p.id]);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const d = pv.data;
  const decide = (accept: boolean, force = false) => { setBusy(true); api.hubProposalDecide(ws, project, p.id, accept, force).then(onDone, (e) => { setErr(String(e)); setBusy(false); }); };
  return (
    <Modal title={`Proposed change: ${p.fields?.path}`} onClose={onClose} foot={<>
      <button className="btn ghost" onClick={() => decide(false)} disabled={busy}>Decline</button>
      <span style={{ flex: 1 }} />
      <button className="btn ghost" onClick={() => download(ws, p.id, String(p.fields?.path))}>Download it</button>
      {d && !d.base_matches && d.exists
        ? <button className="btn primary" disabled={busy} onClick={() => decide(true, true)}>Accept anyway</button>
        : <button className="btn primary" disabled={busy || !d} onClick={() => decide(true)}>Accept into my project</button>}
    </>}>
      <div className="col" style={{ gap: 8 }}>
        {(err || pv.error) && <ErrBox error={err || String(pv.error)} />}
        {p.fields?.note && <div className="small">“{p.fields.note}”</div>}
        {d && <div className="small">{d.exists ? `Replaces your ${fmt(d.old_size)} file with ${fmt(d.new_size)}.` : `New file, ${fmt(d.new_size)}.`} Accepting writes it into your project folder and updates the shared copy; nothing is committed.</div>}
        {d?.same && <div className="small dim">It's identical to your current file.</div>}
        {d && d.exists && !d.base_matches && <div className="small" style={{ color: "var(--gold)" }}>Your copy changed since they started from it. Compare before accepting.</div>}
        {d && (d.old_text != null || d.new_text != null) && (
          <div className="row" style={{ gap: 8, alignItems: "stretch" }}>
            {d.exists && <pre className="file-pane"><b className="dim">yours</b>{"\n"}{d.old_text ?? "(not text)"}</pre>}
            <pre className="file-pane"><b className="dim">proposed</b>{"\n"}{d.new_text ?? "(not text)"}</pre>
          </div>
        )}
      </div>
    </Modal>
  );
}

export default function ProjectFiles({ ws, b, project, local, me, onErr, reload }: { ws: string; b: Row; project: string; local?: string; me: string; onErr: (e: string) => void; reload: () => void }) {
  const files = useAsync(() => api.hubFiles(ws, project), [ws, project], ["hub-changed"]);
  const [dir, setDir] = useState("");
  const [q, setQ] = useState("");
  const [preview, setPreview] = useState<Row | null>(null);
  const [busy, setBusy] = useState("");
  const [review, setReview] = useState<Row | null>(null);
  const [newPath, setNewPath] = useState("");
  const [note, setNote] = useState("");
  const owner = !!local && can(b.role, "editor");
  const propose = can(b.role, "contributor");
  const name = (id?: string) => (b.members ?? []).find((m: Row) => m.user_id === id)?.name ?? "someone";
  const fail = (e: unknown) => { setBusy(""); onErr(String(e)); };

  const send = async (path: string, base: string | null, f?: File) => {
    if (!f) return;
    setBusy(`Sending ${f.name}…`);
    try { await api.hubPropose(ws, project, path, base, note, await readData(f)); setNote(""); setNewPath(""); setBusy(""); reload(); } catch (e) { fail(e); }
  };
  const share = () => { setBusy("Sharing files…"); api.hubFilesShare(local!).then((r) => { setBusy(""); setPreview(null); onErr(""); files.reload?.(); reload(); setBusy(`Shared: ${r.uploaded} uploaded, ${r.removed} removed`); }, fail); };

  const list: Row[] = files.data ?? [];
  const prefix = dir ? dir + "/" : "";
  const inDir = list.filter((f) => String(f.fields?.path).startsWith(prefix) && (!q || String(f.fields?.path).toLowerCase().includes(q.toLowerCase())));
  const sub = new Map<string, number>();
  const here: Row[] = [];
  for (const f of inDir) { const rest = String(f.fields.path).slice(prefix.length); const i = rest.indexOf("/"); if (i < 0 || q) here.push(f); else sub.set(rest.slice(0, i), (sub.get(rest.slice(0, i)) ?? 0) + 1); }
  here.sort((x, y) => String(x.fields.path).localeCompare(String(y.fields.path)));
  const crumbs = dir ? dir.split("/") : [];
  const props: Row[] = (b.records ?? []).filter((r: Row) => r.kind === "proposal" && r.project_id === project).sort((x: Row, y: Row) => String(y.updated_at).localeCompare(String(x.updated_at)));
  const open = props.filter((p) => p.fields?.status === "open");

  return (
    <div className="col" style={{ gap: 12 }}>
      <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
        <span className="dim small">{list.length ? `${list.length} shared files` : "This project's files aren't shared yet."}{busy && ` · ${busy}`}</span>
        {owner && <button className="btn sm" style={{ marginLeft: "auto" }} onClick={() => api.hubFilesPreview(local!).then(setPreview, fail)}>{list.length ? "Update shared files" : "Share project files"}</button>}
      </div>

      {open.length > 0 && (
        <div className="card" style={{ padding: 12 }}>
          <div className="label" style={{ marginBottom: 8 }}>Waiting for {owner ? "you" : "the owner"} · {open.length} proposed change{open.length === 1 ? "" : "s"}</div>
          {open.map((p) => (
            <div key={p.id} className="row" style={{ gap: 8, alignItems: "center", padding: "4px 0" }}>
              <span className="mono small" style={{ flex: 1 }}>{p.fields?.path}</span>
              <span className="dim small">{name(p.created_by)} · {p.fields?.base ? "change" : "new file"} · {ago(p.updated_at)}</span>
              {owner ? <button className="btn sm primary" onClick={() => setReview(p)}>Review</button>
                : <button className="btn sm ghost" onClick={() => download(ws, p.id, String(p.fields?.path)).catch(fail)}>Download</button>}
              {p.created_by === me && !owner && <button className="btn sm ghost" onClick={() => api.hubEdit(ws, "delete", "proposal", project, null, p.id).then(reload, fail)}>Withdraw</button>}
            </div>
          ))}
        </div>
      )}

      {list.length > 0 && (
        <>
          <div className="row wrap" style={{ gap: 6, alignItems: "center" }}>
            <button className="btn sm ghost" onClick={() => setDir("")}>root</button>
            {crumbs.map((c, i) => <span key={i} className="row" style={{ gap: 6 }}><span className="dim">/</span><button className="btn sm ghost" onClick={() => setDir(crumbs.slice(0, i + 1).join("/"))}>{c}</button></span>)}
            <input className="input" style={{ marginLeft: "auto", width: 240 }} placeholder="Find a file…" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Find a file" />
          </div>
          {propose && <input className="input" placeholder="Note for your next proposed change (optional)" value={note} onChange={(e) => setNote(e.target.value)} aria-label="Proposal note" />}
          <div className="col" style={{ maxHeight: 520, overflow: "auto" }}>
            {[...sub.entries()].sort().map(([d, n]) => <div key={d} className="li click" onClick={() => setDir(prefix + d)}><span>📁 {d}</span><span className="dim small mono" style={{ marginLeft: "auto" }}>{n} files</span></div>)}
            {here.slice(0, 1000).map((f) => (
              <div key={f.id} className="li" style={{ gap: 8 }}>
                <span className="mono small">{q ? f.fields.path : String(f.fields.path).slice(prefix.length)}</span>
                <span className="dim small mono" style={{ marginLeft: "auto" }}>{fmt(Number(f.fields?.size ?? 0))}</span>
                <button className="btn sm ghost" onClick={() => download(ws, f.id, String(f.fields.path)).catch(fail)}>Download</button>
                {propose && !owner && <label className="btn sm ghost" style={{ cursor: "pointer" }} title="Send your edited version for the owner to review">Propose change<input type="file" hidden onChange={(e) => { send(String(f.fields.path), String(f.fields.sha256), e.target.files?.[0]); e.target.value = ""; }} /></label>}
              </div>
            ))}
            {!sub.size && !here.length && <div className="dim small">No files match.</div>}
          </div>
        </>
      )}

      {propose && !owner && list.length > 0 && (
        <div className="row wrap" style={{ gap: 8, alignItems: "center" }}>
          <input className="input" placeholder={`New file path, e.g. ${prefix || "art/"}cover.png`} value={newPath} onChange={(e) => setNewPath(e.target.value)} aria-label="New file path" style={{ minWidth: 280 }} />
          <label className={`btn sm ${newPath.trim() ? "" : "disabled"}`} style={{ cursor: newPath.trim() ? "pointer" : "default" }}>Propose new file
            <input type="file" hidden disabled={!newPath.trim()} onChange={(e) => { send(newPath.trim(), null, e.target.files?.[0]); e.target.value = ""; }} /></label>
        </div>
      )}

      {props.length > open.length && (
        <details><summary className="dim small">Decided proposals ({props.length - open.length})</summary>
          {props.filter((p) => p.fields?.status !== "open").map((p) => <div key={p.id} className="dim small" style={{ padding: "2px 0" }}><span className="mono">{p.fields?.path}</span> · {name(p.created_by)} · {p.fields?.status} {ago(p.updated_at)}</div>)}
        </details>
      )}

      {!list.length && !owner && <Empty>{can(b.role, "admin") ? "To share files, share this project from the PC that has it (TEAM › Share a local project), then open FILES there." : "The owner hasn't shared this project's files yet."}</Empty>}

      {preview && (
        <Modal title="Share this project's files with the workspace" onClose={() => setPreview(null)}
          foot={<><button className="btn ghost" onClick={() => setPreview(null)}>Cancel</button><button className="btn primary" disabled={!!busy} onClick={share}>Share</button></>}>
          <div className="col" style={{ gap: 6 }}>
            <div className="small"><b>{preview.files}</b> files: {preview.new} new, {preview.changed} changed, {preview.removed} to remove · {fmt(preview.upload_bytes)} to upload.</div>
            <div className="small">Teammates in this workspace can download them and propose changes; nothing changes in your folder unless you accept.</div>
            <div className="small">Never sent: {(preview.never as string[]).join("; ")}.</div>
            {preview.excluded > 0 && <div className="small" style={{ color: "var(--gold)" }}>{preview.excluded} secret-looking file{preview.excluded === 1 ? " is" : "s are"} left out.</div>}
            {(preview.too_large as string[]).length > 0 && <div className="small dim">Too large to share: {(preview.too_large as string[]).slice(0, 5).join(", ")}{(preview.too_large as string[]).length > 5 ? "…" : ""}</div>}
            {preview.capped && <div className="small" style={{ color: "var(--gold)" }}>Only the first 5,000 files are shared.</div>}
          </div>
        </Modal>
      )}
      {review && <Review ws={ws} project={project} p={review} onClose={() => setReview(null)} onDone={() => { setReview(null); files.reload?.(); reload(); }} />}
    </div>
  );
}
