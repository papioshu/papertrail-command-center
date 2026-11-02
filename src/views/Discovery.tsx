import { useEffect, useMemo, useState } from "react";
import { api, on, type Row } from "../api";
import { ago, Card, Empty, ErrBox, Modal, useAsync } from "../ui";

/* All text here comes from public feeds: rendered as plain text, never executed or launched. */
export default function Discovery() {
  const d = useAsync(() => api.discoveries(), [], ["discovery-updated"]);
  const projects = useAsync(() => api.projects(true), []);
  const [status, setStatus] = useState("new");
  const [proj, setProj] = useState("");
  const [source, setSource] = useState("");
  const [q, setQ] = useState("");
  const [running, setRunning] = useState(false);
  const [compare, setCompare] = useState<string[]>([]);
  const [showCompare, setShowCompare] = useState(false);
  const [bos, setBos] = useState<{ item: Row; project: string; preview?: Row; done?: string; error?: string } | null>(null);
  const [review, setReview] = useState<{ item: Row; project: string; text: string; done?: boolean } | null>(null);

  useEffect(() => {
    const off = on("index-status", (s: Row) => setRunning(s.state === "discovery"));
    return () => { off.then((f) => f()); };
  }, []);

  const items: Row[] = d.data?.items ?? [];
  const pname = (id: string) => projects.data?.find((p) => p.id === id)?.name ?? id;
  const list = useMemo(() => items.filter((x) => {
    const ps: string[] = JSON.parse(x.projects || "[]");
    return (status === "all" || x.status === status) && (!source || x.source === source) &&
      (!proj || ps.includes(proj) || x.project_link === proj) &&
      (!q || `${x.name} ${x.description} ${x.topics}`.toLowerCase().includes(q.toLowerCase()));
  }), [items, status, source, proj, q]);
  const set = (id: string, st?: string, link?: string) => api.setDiscovery(id, st, link).then(d.reload);

  async function openBos(item: Row, project: string) {
    const s = { item, project };
    setBos(s);
    try { setBos({ ...s, preview: await api.bosPreview(item.id, project) }); } catch (e) { setBos({ ...s, error: String(e) }); }
  }

  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="page-head">
        <div>
          <h1>Discovery</h1>
          <div className="sub">
            Matched to the dependencies your projects use · last refresh {d.data?.last ? ago(d.data.last) : "never"} · GitHub search (new repos, last 180 days) + npm major releases
          </div>
        </div>
        <div className="right">
          {compare.length > 1 && <button className="btn" onClick={() => setShowCompare(true)}>Compare {compare.length}</button>}
          <button className="btn primary" disabled={running} onClick={() => { setRunning(true); api.discoveryRefresh(); }}>{running ? "Fetching…" : "Refresh now"}</button>
        </div>
      </div>
      {(d.data?.errors ?? []).length > 0 && <div className="notice">Last refresh reported: {(d.data!.errors as string[]).slice(0, 3).join(" · ")}</div>}
      <Card title="Filter">
        <div className="row wrap">
          <div className="seg">{["new", "saved", "dismissed", "all"].map((s) => <button key={s} className={status === s ? "on" : ""} onClick={() => setStatus(s)}>{s.toUpperCase()}</button>)}</div>
          <div className="seg">{["", "github", "npm"].map((s) => <button key={s} className={source === s ? "on" : ""} onClick={() => setSource(s)}>{s ? s.toUpperCase() : "ALL SOURCES"}</button>)}</div>
          <select className="input" value={proj} onChange={(e) => setProj(e.target.value)}>
            <option value="">Any project</option>
            {(projects.data ?? []).map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
          </select>
          <input className="input" placeholder="search" value={q} onChange={(e) => setQ(e.target.value)} />
          <span className="dim small mono">{list.length} shown</span>
        </div>
      </Card>
      {d.error && <ErrBox error={d.error} />}
      {!items.length && !d.loading && <Empty>Nothing fetched yet. Refresh now pulls from GitHub and npm (needs internet; results are cached for offline use).</Empty>}
      <div className="tickets">
        {list.map((x) => {
          const ps: string[] = JSON.parse(x.projects || "[]");
          return (
            <div key={x.id} className="card" style={{ display: "flex", flexDirection: "column" }}>
              <div className="card-head">
                <span className={`chip ${x.source === "npm" ? "warn" : ""}`}>{x.source === "npm" ? "release" : "repo"}</span>
                <span className="ellipsis" style={{ fontWeight: 600 }} title={x.name}>{x.name}</span>
                <div className="right"><span className="mono small" title="relevance score; see reason">{Number(x.score).toFixed(1)}</span>
                  <input type="checkbox" title="compare" checked={compare.includes(x.id)} onChange={(e) => setCompare(e.target.checked ? [...compare, x.id] : compare.filter((c) => c !== x.id))} /></div>
              </div>
              <div className="card-body col" style={{ gap: 6, flex: 1 }}>
                <div className="small clamp3">{x.description || <span className="dim">No description</span>}</div>
                <div className="row wrap">
                  <span className={`chip ${String(x.pricing).startsWith("Free") ? "ok" : "warn"}`}>{x.pricing}</span>
                  {x.stars != null && <span className="chip">{x.stars}★</span>}
                  {x.created_at && <span className="chip" title="repository created">new {String(x.created_at).slice(0, 10)}</span>}
                  {x.pushed_at && <span className="chip" title="last push">push {ago(x.pushed_at)}</span>}
                </div>
                <div className="dim small">{x.reason}</div>
                <div className="row wrap">{ps.map((p) => <span key={p} className="chip active">{pname(p)}</span>)}{x.project_link && !ps.includes(x.project_link) && <span className="chip active">{pname(x.project_link)} (linked)</span>}</div>
                <div className="mono dim small">Platform: unknown · license from {x.source === "npm" ? "npm metadata" : "GitHub API"}</div>
                <div className="row wrap" style={{ marginTop: "auto" }}>
                  <button className="btn sm" onClick={() => api.openUrl(x.url)}>Open source</button>
                  {x.status !== "saved" && <button className="btn sm" onClick={() => set(x.id, "saved")}>Save</button>}
                  {x.status !== "dismissed" ? <button className="btn sm ghost" onClick={() => set(x.id, "dismissed")}>Dismiss</button> : <button className="btn sm ghost" onClick={() => set(x.id, "new")}>Restore</button>}
                  <select className="input" style={{ padding: "2px 4px", fontSize: 11 }} value="" onChange={(e) => {
                    const [act, pid] = e.target.value.split("|");
                    if (act === "link") set(x.id, undefined, pid);
                    if (act === "bos") openBos(x, pid);
                    if (act === "review") setReview({ item: x, project: pid, text: `Review ${x.name} for ${pname(pid)}: ${x.url}` });
                  }}>
                    <option value="">More…</option>
                    <optgroup label="Associate with project">{(projects.data ?? []).map((p) => <option key={p.id} value={`link|${p.id}`}>{p.name}</option>)}</optgroup>
                    <optgroup label="Add to Book of Secrets">{(projects.data ?? []).filter((p) => p.vault_dir).map((p) => <option key={p.id} value={`bos|${p.id}`}>{p.name}</option>)}</optgroup>
                    <optgroup label="Create review task">{(projects.data ?? []).map((p) => <option key={p.id} value={`review|${p.id}`}>{p.name}</option>)}</optgroup>
                  </select>
                </div>
              </div>
            </div>
          );
        })}
      </div>
      {showCompare && (
        <Modal title="Compare" onClose={() => setShowCompare(false)}>
          <table className="t">
            <thead><tr><th style={{ width: 120 }}>Field</th>{compare.map((id) => <th key={id}>{items.find((x) => x.id === id)?.name}</th>)}</tr></thead>
            <tbody>
              {["description", "license", "pricing", "stars", "created_at", "pushed_at", "topics", "reason", "score"].map((k) => (
                <tr key={k}><td className="label">{k}</td>{compare.map((id) => <td key={id} style={{ whiteSpace: "normal", height: "auto", padding: 8 }}>{String(items.find((x) => x.id === id)?.[k] ?? "—")}</td>)}</tr>
              ))}
            </tbody>
          </table>
        </Modal>
      )}
      {bos && (
        <Modal title={`Add to Book of Secrets · ${pname(bos.project)}`} onClose={() => setBos(null)} foot={bos.done ? <span className="chip ok">Appended to {bos.done}</span> : (
          <button className="btn primary" disabled={!bos.preview} onClick={() => api.bosAppend(bos.item.id, bos.project).then((p) => setBos({ ...bos, done: p }), (e) => setBos({ ...bos, error: String(e) }))}>Append this entry</button>
        )}>
          {bos.error && <ErrBox error={bos.error} />}
          {bos.preview && (
            <div className="col">
              <div className="small">{bos.preview.exists ? "Appends to" : "Creates"} <span className="mono">{bos.preview.path}</span>. Nothing else in the vault changes.</div>
              <pre className="doc">{bos.preview.text}</pre>
            </div>
          )}
        </Modal>
      )}
      {review && (
        <Modal title={`Review task · ${pname(review.project)}`} onClose={() => setReview(null)} foot={review.done ? <span className="chip ok">Added to the project's review list</span> : (
          <button className="btn primary" onClick={() => api.addReviewTask(review.project, review.text, review.item.id).then(() => setReview({ ...review, done: true }))}>Create</button>
        )}>
          <div className="col">
            <textarea className="input" rows={3} value={review.text} onChange={(e) => setReview({ ...review, text: e.target.value })} />
            <span className="muted small">Stored in this app (Project › Review tasks). Your TASKS.md is not modified.</span>
          </div>
        </Modal>
      )}
    </div>
  );
}
