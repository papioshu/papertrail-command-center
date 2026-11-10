// Boards on iPhone/iPad (D050): the same BoardCanvas as the PC, built separately (vite.board.config.ts) with this
// module standing in for "../api". It talks to the hub's HTTP API with the phone page's device token (same origin).
export type Row = Record<string, any>;

const token = () => localStorage.getItem("ptcc-team-token");
const recs = new Map<string, Row>(); // record id -> latest record from the change feed
let since = 0, feedWs = "";

async function http(method: string, path: string, body?: BodyInit, json = true) {
  const headers: Record<string, string> = { Authorization: "Bearer " + token() };
  if (json && body) headers["Content-Type"] = "application/json";
  const r = await fetch(path, { method, headers, body });
  if (r.status === 401) throw new Error("This device was signed out. Open a new invite link on the team page.");
  const isJson = (r.headers.get("Content-Type") || "").includes("json");
  const v = isJson ? await r.json() : await r.blob();
  if (!r.ok) throw new Error(({ forbidden: "Your role can't draw on boards.", not_found: "That isn't there any more.", deleted: "That board was deleted." } as Record<string, string>)[v?.error] ?? String(v?.error ?? r.status));
  return v;
}

// http over Tailscale isn't a secure context (no crypto.randomUUID); getRandomValues is there.
function uuid() {
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 15) | 64; b[8] = (b[8] & 63) | 128;
  const h = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

/** Pulls the change feed into `recs`; true if anything changed. */
export async function pull(ws: string): Promise<boolean> {
  if (ws !== feedWs) { recs.clear(); since = 0; feedWs = ws; }
  let changed = false;
  for (;;) {
    const r = await http("GET", `/v1/workspaces/${ws}/changes?since=${since}&limit=500`);
    for (const x of r.changes) { recs.set(x.id, x); changed = true; }
    since = r.next;
    if (!r.more) return changed;
  }
}
export const records = () => [...recs.values()].filter((r) => !r.deleted);

const dataUrl = (b: Blob) => new Promise<string>((ok, no) => { const r = new FileReader(); r.onload = () => ok(String(r.result)); r.onerror = () => no(new Error("Couldn't read that file.")); r.readAsDataURL(b); });

async function op(ws: string, action: string, kind: string, project: string, fields: Row | null, id?: string) {
  const cur = id ? recs.get(id) : undefined;
  const r = await http("POST", `/v1/workspaces/${ws}/ops`, JSON.stringify({
    op_id: uuid(), action, kind, record_id: id ?? uuid(), project_id: project, base_version: cur ? cur.version : 0, fields: fields ?? {},
  }));
  recs.set(r.record.id, r.record); // the next edit starts from this version
  return r.record.id as string;
}

let live: number | undefined;
const listeners = new Set<() => void>();

export const api = {
  hubBoardEls: async (ws: string, board: string) => {
    await pull(ws);
    return records().filter((r) => r.kind === "board_el" && r.fields?.board === board).map((r) => ({ id: r.id, data: r.fields?.data ?? null }));
  },
  hubEdit: (ws: string, action: string, kind: string, project: string, fields: Row | null, id?: string) => op(ws, action, kind, project, fields, id),
  hubFile: async (ws: string, id: string) => dataUrl(await http("GET", `/v1/workspaces/${ws}/media/${id}`)),
  hubUpload: async (ws: string, project: string, name: string, caption: string, folder: string, data: string, board?: string) => {
    const bytes = await (await fetch(data)).arrayBuffer();
    // The hub fingerprints the upload itself (no crypto.subtle over http).
    const up = await http("PUT", `/v1/workspaces/${ws}/projects/${project}/blobs/auto`, bytes, false);
    return op(ws, "create", "media", project, { sha256: up.sha256, name, caption: caption.slice(0, 1000), folder, ...(board ? { board } : {}) });
  },
  // While a board is open: pull every 1.5 s and tell the canvas when something arrived.
  hubLive: async (on: boolean) => {
    window.clearInterval(live);
    if (on) live = window.setInterval(() => { if (!document.hidden && feedWs) pull(feedWs).then((c) => c && listeners.forEach((f) => f()), () => {}); }, 1500);
  },
};

export function on(event: string, cb: () => void): Promise<() => void> {
  if (event !== "hub-changed") return Promise.resolve(() => {});
  listeners.add(cb);
  return Promise.resolve(() => { listeners.delete(cb); });
}
