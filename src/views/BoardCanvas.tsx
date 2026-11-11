import { useCallback, useEffect, useRef, useState } from "react";
import {
  CaptureUpdateAction,
  convertToExcalidrawElements,
  Excalidraw,
  exportToBlob,
  loadFromBlob,
  MainMenu,
  reconcileElements,
  restoreElements,
  serializeAsJSON,
  viewportCoordsToSceneCoords,
} from "@excalidraw/excalidraw";
import type { BinaryFileData, ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import "@excalidraw/excalidraw/index.css";
import { api, on, type Row } from "../api";

/** One board's canvas (D044), loaded only when a board is opened. Every Excalidraw element is its own hub record
 *  (kind board_el, last writer wins), so edits merge per element and reach the other members live: while a board is
 *  open the PC syncs every second. Pasted or dropped images are uploaded as workspace media (folder "Boards"), stored
 *  once by hash on the hub, and the element points at that media record. */

// Fonts ship with the app (scripts/excalidraw-fonts.mjs); the CDN fallback is blocked by the CSP.
(window as unknown as { EXCALIDRAW_ASSET_PATH: string }).EXCALIDRAW_ASSET_PATH = import.meta.env.BASE_URL + "excalidraw-assets/";

// Excalidraw's dark theme draws the canvas through invert(93%) hue-rotate(180deg), so colors are given pre-inverted:
// these show as near-black (#121212), green-black (#151815), cream paper (#d8cfc0), its ink (#2a2437), light ink (#ece8fb).
const BG = { grid: "#ffffff", blackboard: "#f6faf7" } as const;
const NOTE_FILL = "#2c2110";
const NOTE_INK = "#ebe4fb";
const INK = "#050117";
const EL_MAX = 256 * 1024;

type El = { id: string; version: number; isDeleted?: boolean; type: string; fileId?: string | null } & Record<string, unknown>;

const mimeOf = (name: string) => (/\.png$/i.test(name) ? "image/png" : /\.jpe?g$/i.test(name) ? "image/jpeg" : /\.gif$/i.test(name) ? "image/gif" : /\.webp$/i.test(name) ? "image/webp" : /\.svg$/i.test(name) ? "image/svg+xml" : "image/png");
const dataUrl = (b: Blob) => new Promise<string>((ok, no) => { const r = new FileReader(); r.onload = () => ok(String(r.result)); r.onerror = () => no(new Error("Couldn't read that file.")); r.readAsDataURL(b); });
// Docs (D048): PDFs get a page-1 preview, Word-type files a card, text/Markdown becomes a text note. Clicking a doc's
// link saves the file from workspace media.
const DOC = /\.(pdf|docx?|odt|rtf|pages|md|markdown|txt)$/i;
const TEXT = /\.(md|markdown|txt)$/i;
const DOC_LINK = "ptcc-doc:";
const size = (n: number) => (n < 1024 * 1024 ? `${Math.max(1, Math.round(n / 1024))} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`);
async function pdfPreview(file: File): Promise<Blob> {
  const pdfjs = await import("pdfjs-dist");
  pdfjs.GlobalWorkerOptions.workerSrc = (await import("pdfjs-dist/build/pdf.worker.min.mjs?url")).default;
  const pdf = await pdfjs.getDocument({ data: new Uint8Array(await file.arrayBuffer()), isEvalSupported: false }).promise;
  const page = await pdf.getPage(1);
  const vp = page.getViewport({ scale: 600 / page.getViewport({ scale: 1 }).width });
  const cv = document.createElement("canvas");
  cv.width = Math.ceil(vp.width); cv.height = Math.ceil(vp.height);
  await page.render({ canvasContext: cv.getContext("2d")!, viewport: vp }).promise;
  pdf.destroy();
  return new Promise((ok, no) => cv.toBlob((b) => (b ? ok(b) : no(new Error("Couldn't draw that PDF page."))), "image/png"));
}
// Excalidraw colors a cursor by hashing the collaborator's id (it ignores `color`), so each person on the board gets one
// of these ids, picked to hash to well-spaced hues: violet, teal, green, amber, coral, pink, blue, lime.
const CURSOR_IDS = ["ptcc-5", "ptcc-11", "ptcc-18", "ptcc-52", "ptcc-55", "ptcc-44", "ptcc-9", "ptcc-64"];
const stamp = () => new Date().toISOString().slice(0, 16).replace("T", " ").replace(":", "");

export default function BoardCanvas({ ws, board, b, canDraw, onBack, onErr, reload }: { ws: string; board: Row; b: Row; canDraw: boolean; onBack: () => void; onErr: (e: string) => void; reload: () => void }) {
  const [xapi, setXapi] = useState<ExcalidrawImperativeAPI | null>(null);
  const [status, setStatus] = useState("");
  const [versionsOpen, setVersionsOpen] = useState(false);
  const [label, setLabel] = useState("");
  const [restoring, setRestoring] = useState("");
  const recOf = useRef(new Map<string, string>()); // element id -> board_el record id
  const known = useRef(new Map<string, number>()); // element id -> last version sent or received
  const files = useRef(new Set<string>()); // media record ids already handed to the canvas
  const chain = useRef<Promise<unknown>>(Promise.resolve());
  const timer = useRef<number | undefined>(undefined);
  const title = String(board.fields?.title ?? "Board");
  const bg = board.fields?.bg === "blackboard" ? "blackboard" : "grid";
  const project = String(board.project_id);
  // The board covers the page's error box, so errors also show in the board bar.
  const fail = useCallback((e: unknown) => { setStatus(String(e)); onErr(String(e)); }, [onErr]);
  const versions = (b.records ?? []).filter((r: Row) => r.kind === "media" && r.fields?.board === board.id && /\.excalidraw$/.test(String(r.fields?.name))).sort((x: Row, y: Row) => String(y.updated_at).localeCompare(String(x.updated_at)));
  const mediaName = (id: string) => String((b.records ?? []).find((r: Row) => r.id === id)?.fields?.name ?? "image.png");

  // Images: fetched from the hub one at a time, only for elements on this board.
  const loadFiles = useCallback(async (x: ExcalidrawImperativeAPI, els: readonly El[]) => {
    for (const el of els) {
      const id = el.fileId;
      if (el.type !== "image" || !id || el.isDeleted || files.current.has(id)) continue;
      files.current.add(id);
      try {
        const name = mediaName(id);
        const url = (await api.hubFile(ws, id, name)) as string;
        x.addFiles([{ id, dataURL: url, mimeType: mimeOf(name), created: Date.now() } as unknown as BinaryFileData]);
      } catch {
        files.current.delete(id);
      }
    }
  }, [ws, b.records]); // eslint-disable-line react-hooks/exhaustive-deps

  const fetchEls = useCallback(async () => {
    const out: El[] = [];
    for (const r of await api.hubBoardEls(ws, board.id)) {
      try {
        const el = JSON.parse(r.data ?? "") as El;
        if (el && typeof el.id === "string") { recOf.current.set(el.id, r.id); out.push(el); }
      } catch { /* a malformed element is skipped, never fatal */ }
    }
    return out;
  }, [ws, board.id]);

  const initial = useRef<Promise<unknown> | null>(null);
  if (!initial.current) {
    initial.current = fetchEls().then((els) => {
      for (const el of els) known.current.set(el.id, el.version);
      return {
        elements: restoreElements(els as never, null),
        appState: { viewBackgroundColor: BG[bg], gridModeEnabled: bg === "grid", gridSize: 20, gridStep: 5, theme: "dark", currentItemStrokeColor: INK, name: title },
        scrollToContent: true,
      };
    }, (e) => { fail(e); return { elements: [], appState: { viewBackgroundColor: BG[bg], theme: "dark" } }; });
  }

  useEffect(() => { if (xapi) initial.current?.then(() => loadFiles(xapi, xapi.getSceneElementsIncludingDeleted() as unknown as El[])); }, [xapi, loadFiles]);

  // Live: sync every second while open; apply other members' newer elements as they arrive.
  useEffect(() => {
    api.hubLive(true).catch(() => {});
    return () => { api.hubLive(false).catch(() => {}); };
  }, []);
  useEffect(() => {
    if (!xapi) return;
    const un = on("hub-changed", async () => {
      const incoming = (await fetchEls()).filter((el) => el.version > (known.current.get(el.id) ?? -1));
      if (!incoming.length) return;
      for (const el of incoming) known.current.set(el.id, el.version);
      const merged = reconcileElements(xapi.getSceneElementsIncludingDeleted(), incoming as never, xapi.getAppState());
      xapi.updateScene({ elements: merged, captureUpdate: CaptureUpdateAction.NEVER });
      loadFiles(xapi, incoming);
    });
    return () => { un.then((f) => f()); };
  }, [xapi, fetchEls, loadFiles]);

  // Local edits: after a short pause, each element whose version moved is written (one record per element).
  const push = useCallback(() => {
    if (!xapi || !canDraw) return;
    chain.current = chain.current.then(async () => {
      let n = 0;
      for (const el of xapi.getSceneElementsIncludingDeleted() as unknown as El[]) {
        if (known.current.get(el.id) === el.version) continue;
        const data = JSON.stringify(el);
        if (data.length > EL_MAX) { setStatus("One drawing is too large to share; split it up."); continue; }
        const rec = recOf.current.get(el.id);
        if (!rec && el.isDeleted) { known.current.set(el.id, el.version); continue; }
        known.current.set(el.id, el.version);
        try {
          if (rec) await api.hubEdit(ws, "update", "board_el", project, { data }, rec);
          else recOf.current.set(el.id, await api.hubEdit(ws, "create", "board_el", project, { board: board.id, data }));
        } catch (e) {
          known.current.delete(el.id); // not saved: the next pass sends it again
          throw e;
        }
        n++;
      }
      if (n) setStatus("Saved");
    }).catch((e) => { setStatus(""); fail(e); window.clearTimeout(timer.current); timer.current = window.setTimeout(() => push(), 3000); });
  }, [xapi, canDraw, ws, project, board.id, fail]); // eslint-disable-line react-hooks/exhaustive-deps
  const onChange = useCallback(() => { window.clearTimeout(timer.current); timer.current = window.setTimeout(push, 250); }, [push]);

  // Presence (B3): who else has this board open, with their cursor and selection. Twice a second while visible.
  const pointer = useRef<{ x: number; y: number } | null>(null);
  useEffect(() => {
    if (!xapi) return;
    const tick = async () => {
      if (document.hidden) return;
      const p = pointer.current;
      const sel = Object.keys(xapi.getAppState().selectedElementIds ?? {});
      const r = await api.hubPresence(ws, board.id, p ? { x: p.x, y: p.y, sel } : {}).catch(() => null);
      if (!r) return;
      // Ordered by user id, so everyone on the board gets a different color (up to 8).
      const people = [...r.people].sort((a: Row, c: Row) => String(a.user).localeCompare(String(c.user)));
      const collaborators = new Map(people.map((u: Row, i: number) => [String(u.user), {
        id: CURSOR_IDS[i % CURSOR_IDS.length], username: String(u.name),
        pointer: { x: u.x, y: u.y, tool: "pointer" }, selectedElementIds: Object.fromEntries((u.sel ?? []).map((id: string) => [id, true])),
      }]));
      xapi.updateScene({ collaborators: collaborators as never, captureUpdate: CaptureUpdateAction.NEVER });
    };
    const t = window.setInterval(tick, 500);
    return () => { window.clearInterval(t); api.hubPresence(ws, board.id, { leave: true }).catch(() => {}); };
  }, [xapi, ws, board.id]);
  useEffect(() => () => { window.clearTimeout(timer.current); push(); }, [push]);

  // Pasted / dropped images become workspace media first; the element then names the media record.
  const generateIdForFile = useCallback(async (file: File) => {
    if (file.size > 25 * 1024 * 1024) throw new Error("Images can be at most 25 MB.");
    setStatus("Uploading image…");
    const id = await api.hubUpload(ws, project, file.name || `pasted ${stamp()}.png`, `On board: ${title}`, "Boards", await dataUrl(file));
    files.current.add(id);
    setStatus("Image saved to workspace media");
    reload();
    return id;
  }, [ws, project, title, reload]);

  const addNote = () => {
    if (!xapi) return;
    const st = xapi.getAppState();
    const z = st.zoom.value;
    const x = -st.scrollX + st.width / 2 / z - 110, y = -st.scrollY + st.height / 2 / z - 70;
    const els = convertToExcalidrawElements([{
      type: "rectangle", x, y, width: 220, height: 140, backgroundColor: NOTE_FILL, fillStyle: "solid", strokeColor: NOTE_FILL, roughness: 0,
      roundness: { type: 3 }, label: { text: "Note", strokeColor: NOTE_INK, fontSize: 20 },
    } as never]);
    xapi.updateScene({ elements: [...xapi.getSceneElementsIncludingDeleted(), ...els], appState: { selectedElementIds: { [els[0].id]: true } } as never, captureUpdate: CaptureUpdateAction.IMMEDIATELY });
  };

  // Save a version: a PNG (as seen, also the board's thumbnail) and the editable .excalidraw file, both saved into the
  // workspace's media under "Boards" and tagged with this board, so they survive a rename.
  const exportBoard = async (label = "") => {
    if (!xapi) return;
    setStatus("Saving version…");
    const elements = xapi.getSceneElements();
    const appState = xapi.getAppState();
    const fileMap = xapi.getFiles();
    const name = `${title}${label ? ` — ${label}` : ""} ${stamp()}`;
    const png = await exportToBlob({ elements, appState: { ...appState, exportBackground: true, exportWithDarkMode: true }, files: fileMap, mimeType: "image/png", exportPadding: 32 });
    await api.hubUpload(ws, project, `${name}.png`, `Export of board ${title}`, "Boards", await dataUrl(png), board.id);
    const json = serializeAsJSON(elements, appState, fileMap, "local");
    await api.hubUpload(ws, project, `${name}.excalidraw`, `Editable copy of board ${title}`, "Boards", await dataUrl(new Blob([json], { type: "application/json" })), board.id);
    setStatus("Version saved to MEDIA › Boards");
    reload();
  };
  const save = (label = "") => exportBoard(label).then(() => setLabel(""), (e) => { setStatus(""); fail(e); });

  // Restore: the current board is saved as a version first, then every element is replaced by the saved one for
  // everyone (versions bumped past the live ones, so last-writer-wins keeps the restore).
  const restore = async (v: Row) => {
    if (!xapi) return;
    try {
      await exportBoard("before restore");
      setStatus("Restoring…");
      const url = String(await api.hubFile(ws, v.id, String(v.fields?.name)));
      const bin = atob(url.slice(url.indexOf(",") + 1));
      const scene = await loadFromBlob(new Blob([Uint8Array.from(bin, (ch) => ch.charCodeAt(0))], { type: "application/json" }), null, null);
      const now = xapi.getSceneElementsIncludingDeleted();
      const top = Math.max(0, ...now.map((e) => e.version), ...scene.elements.map((e) => e.version)) + 1;
      const bump = <T extends object>(e: T, extra: object = {}) => ({ ...e, ...extra, version: top, versionNonce: (Math.random() * 2 ** 31) | 0, updated: Date.now() });
      const keep = new Set(scene.elements.map((e) => e.id));
      xapi.addFiles(Object.values(scene.files ?? {}));
      xapi.updateScene({
        elements: [...now.filter((e) => !keep.has(e.id)).map((e) => bump(e, { isDeleted: true })), ...scene.elements.map((e) => bump(e))] as never,
        captureUpdate: CaptureUpdateAction.IMMEDIATELY,
      });
      loadFiles(xapi, scene.elements as unknown as El[]);
      setRestoring(""); setVersionsOpen(false);
      setStatus(`Restored ${String(v.fields?.name).replace(/\.excalidraw$/, "")}`);
    } catch (e) { setStatus(""); fail(e); }
  };

  // Docs dropped or picked: each becomes workspace media plus elements placed where it was dropped (or mid-screen).
  const addDocs = async (list: File[], at?: { x: number; y: number }) => {
    if (!xapi || !list.length) return;
    const st = xapi.getAppState();
    let { x, y } = at ?? { x: -st.scrollX + st.width / 2 / st.zoom.value - 150, y: -st.scrollY + st.height / 2 / st.zoom.value - 100 };
    try {
      for (const f of list) {
        if (f.size > 50 * 1024 * 1024) throw new Error(`${f.name} is over 50 MB.`);
        setStatus(`Adding ${f.name}…`);
        let els;
        if (TEXT.test(f.name)) {
          const text = (await f.text()).slice(0, 8000);
          els = convertToExcalidrawElements([{ type: "text", x, y, text, fontSize: 16, strokeColor: INK } as never]);
        } else {
          const media = await api.hubUpload(ws, project, f.name, `On board: ${title}`, "Boards", await dataUrl(f));
          const label = `📄 ${f.name}\n${size(f.size)} · link ↗ saves it`;
          if (/\.pdf$/i.test(f.name)) {
            const png = await pdfPreview(f).catch(() => null);
            if (png) {
              const pid = await api.hubUpload(ws, project, `${f.name} page 1.png`, `Preview of ${f.name}`, "Boards", await dataUrl(png));
              files.current.add(pid);
              xapi.addFiles([{ id: pid, dataURL: await dataUrl(png), mimeType: "image/png", created: Date.now() } as unknown as BinaryFileData]);
              const img = await createImageBitmap(png);
              const h = Math.round((300 * img.height) / img.width);
              els = convertToExcalidrawElements([
                { type: "image", x, y, width: 300, height: h, fileId: pid, link: DOC_LINK + media } as never,
                { type: "text", x, y: y + h + 8, text: label, fontSize: 14, strokeColor: INK, link: DOC_LINK + media } as never,
              ]);
            }
          }
          els ??= convertToExcalidrawElements([{
            type: "rectangle", x, y, width: 300, height: 90, backgroundColor: NOTE_FILL, fillStyle: "solid", strokeColor: NOTE_FILL, roughness: 0, roundness: { type: 3 },
            link: DOC_LINK + media, label: { text: label, strokeColor: NOTE_INK, fontSize: 16 },
          } as never]);
        }
        xapi.updateScene({ elements: [...xapi.getSceneElementsIncludingDeleted(), ...els], captureUpdate: CaptureUpdateAction.IMMEDIATELY });
        x = Math.max(...els.map((el) => el.x + el.width)) + 40; // next doc to the right
      }
      setStatus(list.length > 1 ? `Added ${list.length} docs` : `Added ${list[0].name}`);
      reload();
    } catch (e) { setStatus(""); fail(e); }
  };
  const onDocDrop = (e: React.DragEvent) => {
    const list = [...e.dataTransfer.files].filter((f) => DOC.test(f.name));
    if (!list.length || !xapi || !canDraw) return; // images and .excalidraw files go to the canvas as before
    e.preventDefault(); e.stopPropagation();
    const p = viewportCoordsToSceneCoords({ clientX: e.clientX, clientY: e.clientY }, xapi.getAppState());
    addDocs(list, { x: p.x, y: p.y });
  };
  // A doc's link saves the file (its media record) through the browser's download.
  const openDoc = useCallback(async (el: { link?: string | null }, ev: CustomEvent<{ nativeEvent: MouseEvent | React.PointerEvent<HTMLCanvasElement> }>) => {
    if (!el.link?.startsWith(DOC_LINK)) return;
    ev.preventDefault();
    const id = el.link.slice(DOC_LINK.length);
    const name = mediaName(id);
    try {
      const a = document.createElement("a");
      a.href = String(await api.hubFile(ws, id, name)); a.download = name; a.click();
      setStatus(`Saved ${name} to Downloads`);
    } catch (e) { fail(e); }
  }, [ws, b.records, fail]); // eslint-disable-line react-hooks/exhaustive-deps

  // Import: adds a .excalidraw file's elements to this board; its images are uploaded as workspace media first.
  const importBoard = async (f: File | undefined) => {
    if (!xapi || !f) return;
    try {
      setStatus("Importing…");
      const scene = await loadFromBlob(f, null, null);
      const remap = new Map<string, string>();
      for (const file of Object.values(scene.files ?? {})) {
        const id = await api.hubUpload(ws, project, `imported ${stamp()}.${(file.mimeType.split("/")[1] ?? "png").replace("jpeg", "jpg").replace("svg+xml", "svg")}`, `Imported into board ${title}`, "Boards", file.dataURL);
        remap.set(file.id, id);
        files.current.add(id);
        xapi.addFiles([{ ...file, id } as BinaryFileData]);
      }
      const have = new Set(xapi.getSceneElementsIncludingDeleted().map((e) => e.id));
      const added = scene.elements.filter((e) => !have.has(e.id) && !e.isDeleted).map((e) => ("fileId" in e && e.fileId && remap.has(e.fileId) ? { ...e, fileId: remap.get(e.fileId) } : e));
      xapi.updateScene({ elements: [...xapi.getSceneElementsIncludingDeleted(), ...(added as never[])], captureUpdate: CaptureUpdateAction.IMMEDIATELY });
      setStatus(`Imported ${added.length} items`);
      reload();
    } catch (e) { setStatus(""); fail(e); }
  };

  return (
    <div className={`board-shell ${bg}`}>
      <div className="board-bar">
        <button className="btn sm ghost" onClick={onBack}>← Boards</button>
        <span className="board-name">{title}</span>
        <span className="chip">{bg === "grid" ? "Dark grid" : "Blackboard"}</span>
        <span className="chip live" title="Changes sync to the workspace every second while this board is open">LIVE</span>
        {!canDraw && <span className="chip">view only</span>}
        <span className="dim small">{status}</span>
        <span style={{ flex: 1 }} />
        {canDraw && <button className="btn sm" onClick={addNote}>+ Note</button>}
        {canDraw && <label className="btn sm" style={{ cursor: "pointer" }} title="PDF, Word, text or Markdown; or drop them on the board">+ Doc<input type="file" multiple accept=".pdf,.doc,.docx,.odt,.rtf,.pages,.md,.markdown,.txt" hidden onChange={(e) => { addDocs([...(e.target.files ?? [])]); e.target.value = ""; }} /></label>}
        {canDraw && <label className="btn sm" style={{ cursor: "pointer" }}>Import<input type="file" accept=".excalidraw,application/json" hidden onChange={(e) => { importBoard(e.target.files?.[0]); e.target.value = ""; }} /></label>}
        {canDraw && <button className="btn sm" onClick={() => setVersionsOpen(!versionsOpen)} aria-expanded={versionsOpen}>Versions{versions.length ? ` (${versions.length})` : ""}</button>}
        {canDraw && <button className="btn sm primary" onClick={() => save()}>Save to workspace</button>}
      </div>
      {versionsOpen && (
        <div className="card board-versions">
          <div className="row" style={{ gap: 6 }}>
            <input className="input" placeholder="Label, e.g. pitch draft" value={label} aria-label="Version label" maxLength={60}
              onChange={(e) => setLabel(e.target.value)} onKeyDown={(e) => e.key === "Enter" && save(label.trim())} style={{ flex: 1 }} />
            <button className="btn sm primary" onClick={() => save(label.trim())}>Save version</button>
          </div>
          {!versions.length ? <div className="dim small">No saved versions yet.</div> : versions.map((v: Row) => (
            <div key={v.id} className="row" style={{ gap: 6, alignItems: "center" }}>
              <span className="small" style={{ flex: 1 }}>{String(v.fields?.name).replace(/\.excalidraw$/, "")}</span>
              {restoring === v.id ? (
                <>
                  <button className="btn sm" style={{ color: "var(--signal)" }} onClick={() => restore(v)}>Restore for everyone</button>
                  <button className="btn sm ghost" onClick={() => setRestoring("")}>Keep</button>
                </>
              ) : <button className="btn sm ghost" onClick={() => setRestoring(v.id)}>Restore</button>}
            </div>
          ))}
        </div>
      )}
      <div className="board-canvas" onDropCapture={onDocDrop} onDragOverCapture={(e) => e.preventDefault()}>
        <Excalidraw
          excalidrawAPI={setXapi}
          initialData={initial.current as never}
          theme="dark"
          viewModeEnabled={!canDraw}
          onChange={onChange}
          onPointerUpdate={({ pointer: p }) => { pointer.current = { x: p.x, y: p.y }; }}
          isCollaborating
          generateIdForFile={generateIdForFile}
          onLinkOpen={openDoc}
          UIOptions={{ canvasActions: { changeViewBackgroundColor: false, toggleTheme: false, loadScene: false, saveToActiveFile: false, export: false } }}
        >
          <MainMenu>
            <MainMenu.DefaultItems.SaveAsImage />
            <MainMenu.DefaultItems.ClearCanvas />
            <MainMenu.DefaultItems.Help />
          </MainMenu>
        </Excalidraw>
      </div>
    </div>
  );
}
