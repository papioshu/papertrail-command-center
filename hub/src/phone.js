// Team board for phones (D040): served by the hub, same API and rules as the desktop app.
// shortcut: no offline queue or conflict screens; each change goes straight to the hub (refused changes show an error).
"use strict";
const KEY = "ptcc-team-token";
let token = localStorage.getItem(KEY);
let me = null, ws = "", project = "", tab = "tasks";
let feed = { records: [], projects: [], members: [] };
const COLS = [["todo", "To do", "--soft"], ["doing", "In progress", "--violet"], ["blocked", "Blocked", "--gold"], ["done", "Done", "--lime"]];

// Text only, never HTML: nothing a teammate types can run as code.
function el(tag, props, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (k === "on") for (const [ev, f] of Object.entries(v)) n.addEventListener(ev, f);
    else if (k === "style") n.setAttribute("style", v);
    else n[k] = v;
  }
  for (const c of kids.flat()) if (c != null && c !== false) n.append(c instanceof Node ? c : document.createTextNode(String(c)));
  return n;
}
const $ = (id) => document.getElementById(id);
const show = (...nodes) => $("app").replaceChildren(...nodes);
const say = (text) => { $("msg").replaceChildren(text ? el("div", { className: "err" }, text) : ""); };

// http over Tailscale isn't a "secure context", so crypto.randomUUID isn't there; getRandomValues is.
function uuid() {
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 15) | 64; b[8] = (b[8] & 63) | 128;
  const h = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

async function api(method, path, body, raw) {
  const headers = {};
  if (token) headers.Authorization = "Bearer " + token;
  if (!raw) headers["Content-Type"] = "application/json";
  const r = await fetch(path, { method, headers, body: raw ?? (body ? JSON.stringify(body) : undefined) });
  if (r.status === 401 && token) { localStorage.removeItem(KEY); token = null; start(); throw new Error("This phone was signed out. Open a new invite link."); }
  const isJson = (r.headers.get("Content-Type") || "").includes("json");
  const v = isJson ? await r.json() : await r.blob();
  if (!r.ok) throw new Error(friendly(isJson ? v.error : String(r.status)));
  return v;
}
function friendly(e) {
  return ({ forbidden: "Your role can't do that.", not_found: "That isn't there any more.", invalid_invitation: "That invite has expired or was already used. Ask Paper for a new one.", invalid_device_link: "That device link has expired or was already used. Make a new one with + Device on your other device.", conflict: "Someone changed that first. It's been refreshed.", deleted: "That task was deleted.", already_member: "You're already in that workspace." })[e] || e;
}
const run = (f) => async (...a) => { say(""); try { await f(...a); } catch (e) { say(e.message); await load().catch(() => {}); } };

/* ---------- joining ---------- */

function joinScreen(code) {
  const name = el("input", { placeholder: "Your name, as your team will see it", className: "grow", value: localStorage.getItem("ptcc-team-name") || "" });
  const go = run(async () => {
    const r = await api("POST", "/v1/redeem", { token: code, name: name.value.trim(), device_name: "iPhone" });
    if (r.device && r.device.token) { token = r.device.token; localStorage.setItem(KEY, token); }
    localStorage.setItem("ptcc-team-name", name.value.trim());
    history.replaceState(null, "", "/");
    ws = r.workspace_id;
    await start();
  });
  show(el("h1", {}, "JOIN YOUR TEAM"), el("p", { className: "dim" }, token ? "Add this workspace to your account." : "This invite works once. After joining, add this page to your Home Screen (Share › Add to Home Screen)."),
    el("div", { className: "row" }, name), el("p"), el("button", { className: "primary", on: { click: go } }, "Join"));
}

/* ---------- data ---------- */

async function load() {
  const all = []; let since = 0;
  for (;;) { // shortcut: re-reads the whole feed each time; fine for a small team, add a cursor if it grows
    const r = await api("GET", `/v1/workspaces/${ws}/changes?since=${since}&limit=500`);
    all.push(...r.changes); feed.projects = r.projects; since = r.next;
    if (!r.more) break;
  }
  feed.records = all.filter((x) => !x.deleted);
  feed.members = (await api("GET", `/v1/workspaces/${ws}/members`)).members;
  if (!feed.projects.some((p) => p.id === project)) project = feed.projects[0] ? feed.projects[0].id : "";
  render();
}

const nameOf = (id) => (feed.members.find((m) => m.user_id === id) || {}).name || "someone";
const op = (action, kind, rec, fields) => api("POST", `/v1/workspaces/${ws}/ops`, {
  op_id: uuid(), action, kind, record_id: rec ? rec.id : uuid(), project_id: rec ? rec.project_id : project, base_version: rec ? rec.version : 0, fields,
});

/* ---------- screens ---------- */

function taskCard(t) {
  const f = t.fields || {};
  const st = f.status || "todo";
  const mine = f.assignee === me.user_id;
  return el("div", { className: "card" },
    el("div", { style: `font-weight:600;${st === "done" ? "text-decoration:line-through" : ""}` }, f.title),
    f.notes ? el("div", { className: "dim" }, f.notes) : null,
    el("div", { className: "dim" }, f.assignee ? (mine ? "claimed by you" : "claimed by " + nameOf(f.assignee)) : "unclaimed", f.due ? " · due " + f.due : ""),
    el("div", { className: "row", style: "margin-top:8px" },
      !f.assignee ? el("button", { className: "small primary", on: { click: run(async () => { await op("update", "task", t, { assignee: me.user_id }); await load(); }) } }, "Claim") : null,
      mine ? el("button", { className: "small", on: { click: run(async () => { await op("update", "task", t, { assignee: null }); await load(); }) } }, "Unclaim") : null,
      ...COLS.filter(([k]) => k !== st).map(([k, label]) => el("button", { className: "small", on: { click: run(async () => { await op("update", "task", t, { status: k }); await load(); }) } }, label))));
}

function tasks() {
  const list = feed.records.filter((r) => r.kind === "task" && r.project_id === project);
  const title = el("input", { placeholder: "New task…", className: "grow" });
  const add = run(async () => { if (!title.value.trim()) return; await op("create", "task", null, { title: title.value.trim(), status: "todo" }); await load(); });
  return [el("div", { className: "row", style: "margin-top:10px" }, title, el("button", { className: "primary", on: { click: add } }, "Add")),
    ...COLS.map(([k, label, color]) => {
      const items = list.filter((t) => ((t.fields || {}).status || "todo") === k);
      return el("div", {}, el("div", { className: "sec", style: `border-color:var(${color});color:var(${color})` }, el("span", {}, label), el("span", {}, items.length)),
        items.length ? items.map(taskCard) : el("div", { className: "dim" }, "none"));
    })];
}

function thumb(r) {
  const f = r.fields || {};
  const box = el("div", {}, el("div", { className: "dim" }, "loading…"));
  api("GET", `/v1/workspaces/${ws}/media/${r.id}`).then((blob) => {
    const url = URL.createObjectURL(blob);
    const isImg = /\.(png|jpe?g|gif|webp|heic)$/i.test(f.name || ""), isVid = /\.(mp4|mov|webm)$/i.test(f.name || "");
    box.replaceChildren(isImg ? el("img", { src: url, alt: f.name }) : isVid ? el("video", { src: url, controls: true, playsInline: true }) : el("a", { href: url, download: f.name }, "Open file"));
  }, () => box.replaceChildren(el("div", { className: "dim" }, "unavailable")));
  return el("div", {}, box, el("div", { className: "dim" }, (f.folder ? f.folder + " · " : "") + (f.name || "")));
}

function media() {
  const list = feed.records.filter((r) => r.kind === "media" && r.project_id === project);
  const folder = el("input", { placeholder: "Folder (e.g. Renders)", className: "grow", value: localStorage.getItem("ptcc-team-folder") || "" });
  const file = el("input", { type: "file", accept: "image/*,video/*,audio/*,application/pdf", style: "display:none" });
  const pick = el("button", { className: "primary", on: { click: () => file.click() } }, "Upload");
  file.addEventListener("change", run(async () => {
    const fl = file.files[0]; if (!fl) return;
    if (fl.size > 25 * 1024 * 1024) throw new Error("Files can be at most 25 MB.");
    pick.textContent = "Uploading…"; pick.disabled = true;
    // The hub fingerprints the file itself (no crypto.subtle over http).
    const up = await api("PUT", `/v1/workspaces/${ws}/projects/${project}/blobs/auto`, null, await fl.arrayBuffer());
    const fields = { sha256: up.sha256, name: fl.name };
    if (folder.value.trim()) fields.folder = folder.value.trim();
    localStorage.setItem("ptcc-team-folder", folder.value.trim());
    await op("create", "media", null, fields);
    await load();
  }));
  return [el("div", { className: "row", style: "margin-top:10px" }, folder, pick, file),
    list.length ? el("div", { className: "grid", style: "margin-top:10px" }, list.map(thumb)) : el("p", { className: "dim" }, "No media here yet.")];
}

// Boards (D050) open full screen on /board/; draw with Apple Pencil on iPad.
function boards() {
  const list = feed.records.filter((r) => r.kind === "board" && r.project_id === project);
  return [el("p", { className: "dim" }, "Boards open full screen. On iPad, Apple Pencil draws and fingers move the canvas (the board switches to pen mode the first time the Pencil touches it)."),
    ...(list.length ? list.map((b) => el("a", { href: `/board/#${ws}/${b.id}`, style: "display:block;color:inherit;text-decoration:none" },
      el("div", { className: "card" }, el("div", { style: "font-weight:600" }, (b.fields || {}).title || "Board"), el("div", { className: "dim" }, (b.fields || {}).bg === "blackboard" ? "Blackboard" : "Dark grid"))))
      : [el("p", { className: "dim" }, "No boards in this project yet. Paper makes them on the PC (TEAM › BOARDS).")])];
}

/* ---------- Ghost: voice (D052) ---------- */
// The phone's own recognizer hears; the hub's /v1/voice understands and acts with this person's rights; the phone speaks
// the answer. Browsers only allow the microphone on the https:// address.
const SR = window.SpeechRecognition || window.webkitSpeechRecognition;
let wake = null, armed = 0, ghostLog = [];
const voiceOk = () => SR && window.isSecureContext;
function speak(t) { try { speechSynthesis.cancel(); speechSynthesis.speak(new SpeechSynthesisUtterance(t)); } catch { /* no voice: the text still shows */ } }
function beep() {
  try { const a = new (window.AudioContext || window.webkitAudioContext)(), o = a.createOscillator(), g = a.createGain(); o.frequency.value = 880; g.gain.value = 0.08; o.connect(g); g.connect(a.destination); o.start(); o.stop(a.currentTime + 0.12); } catch { /* silent */ }
}
function ghostShow(heard, answer) {
  ghostLog = [{ heard, answer }, ...ghostLog].slice(0, 3);
  const box = $("ghost-out");
  if (box) box.replaceChildren(...ghostLog.map((x) => el("div", { className: "card" }, el("div", { className: "dim" }, x.heard), x.answer ? el("div", {}, x.answer) : null)));
}
async function ask(text) {
  ghostShow(`"${text}"`, "…");
  try {
    const r = await api("POST", "/v1/voice", { text, workspace: ws });
    ghostLog[0].answer = r.say; ghostShow(ghostLog.shift().heard, r.say); speak(r.say);
    load().catch(() => {});
  } catch (e) { ghostLog[0].answer = e.message; ghostShow(ghostLog.shift().heard, e.message); }
}
function listenOnce() {
  if (!voiceOk()) return say(SR ? "Voice needs the secure link (https://). Ask Paper for a new device link." : "This browser can't listen. Use Safari or Chrome.");
  const rec = new SR();
  rec.lang = "en-US"; rec.interimResults = false; rec.maxAlternatives = 1;
  rec.onresult = (e) => ask(e.results[0][0].transcript);
  rec.onerror = (e) => { if (e.error !== "no-speech" && e.error !== "aborted") say("Voice: " + e.error); };
  if (wake) wake.abort();
  rec.onend = () => { if (localStorage.getItem("ptcc-ghost-wake") === "1") startWake(); };
  beep(); ghostShow("Listening…", ""); rec.start();
}
// "Hey Ghost": listens while this page is open and on screen (phones never let a web page listen in the background).
function startWake() {
  if (!voiceOk() || wake || document.hidden) return;
  const rec = new SR();
  rec.lang = "en-US"; rec.continuous = true; rec.interimResults = false;
  rec.onresult = (e) => {
    const t = e.results[e.results.length - 1][0].transcript.trim();
    if (armed > Date.now()) { armed = 0; return ask(t); }
    const m = t.match(/\bghost\b[\s,.!?]*(.*)$/i);
    if (!m) return;
    if (m[1].trim().length > 2) ask(m[1].trim()); else { armed = Date.now() + 8000; beep(); ghostShow("Yes?", ""); }
  };
  rec.onerror = (e) => { if (e.error === "not-allowed") { localStorage.setItem("ptcc-ghost-wake", "0"); say("Microphone access was refused."); render(); } };
  rec.onend = () => { wake = null; if (localStorage.getItem("ptcc-ghost-wake") === "1" && !document.hidden) setTimeout(startWake, 400); };
  wake = rec; rec.start();
}
function stopWake() { const w = wake; wake = null; if (w) w.abort(); }

// "Hey Siri, Ghost": a key for an Apple Shortcut that sends what Siri heard to the hub and speaks the answer.
function siriSetup() {
  return run(async () => {
    const r = await api("POST", "/v1/devices", { name: "Siri shortcut" });
    const field = (label, value) => el("div", { className: "row", style: "margin:6px 0" }, el("span", { className: "dim", style: "min-width:110px" }, label),
      el("input", { value, readOnly: true, className: "grow", on: { focus: (e) => e.target.select() } }));
    show(el("h1", {}, "HEY SIRI, GHOST"),
      el("p", { className: "dim" }, "Works with the phone locked. Tailscale must be on. Treat the key like a password: it acts as you."),
      field("URL", `${location.origin}/v1/voice`), field("Header value", `Bearer ${r.token}`),
      el("ol", { style: "line-height:1.7;padding-left:20px" },
        el("li", {}, "Open Shortcuts › + . Name the shortcut Ghost."),
        el("li", {}, "Add action: Dictate Text."),
        el("li", {}, "Add action: Get Contents of URL. Paste the URL. Show More: Method POST; Headers: add Authorization with the header value above; Request Body JSON: add key text, value Dictated Text."),
        el("li", {}, "Add action: Get Dictionary Value: Value for key say in Contents of URL."),
        el("li", {}, "Add action: Speak Text (Dictionary Value)."),
        el("li", {}, "Say \"Hey Siri, Ghost\", then your command.")),
      el("button", { className: "primary", on: { click: run(load) } }, "Done"));
  });
}

function ghostBar() {
  const on = localStorage.getItem("ptcc-ghost-wake") === "1";
  if (on) setTimeout(startWake, 0); // picks listening back up after a reload or refresh
  const toggle = () => { localStorage.setItem("ptcc-ghost-wake", on ? "0" : "1"); if (on) stopWake(); else startWake(); render(); };
  return [el("div", { className: "row", style: "margin-top:8px" },
    el("button", { className: "primary", on: { click: listenOnce } }, "🎙 Ask Ghost"),
    el("button", { className: on ? "on" : "", style: on ? "border-color:var(--lime);color:var(--lime)" : "", on: { click: toggle }, title: "Listens for \"Hey Ghost\" while this page is open" }, on ? "Hey Ghost: on" : "Hey Ghost: off"),
    /iPhone|iPad|Macintosh/.test(navigator.userAgent) ? el("button", { className: "small", on: { click: siriSetup() } }, "Siri") : null),
    el("div", { id: "ghost-out" }, ghostLog.map((x) => el("div", { className: "card" }, el("div", { className: "dim" }, x.heard), x.answer ? el("div", {}, x.answer) : null)))];
}

function render() {
  const wsSel = el("select", { className: "grow", on: { change: run(async (e) => { ws = e.target.value; project = ""; await load(); }) } },
    me.workspaces.map((w) => el("option", { value: w.id, selected: w.id === ws }, `${w.name} · ${w.title || w.role}`)));
  const pSel = el("select", { className: "grow", on: { change: (e) => { project = e.target.value; render(); } } },
    feed.projects.length ? feed.projects.map((p) => el("option", { value: p.id, selected: p.id === project }, p.name)) : el("option", {}, "No shared projects yet"));
  const tabs = el("div", { className: "row tabs", style: "margin-top:8px" }, ["tasks", "media", "boards"].map((t) => el("button", { className: tab === t ? "on" : "", on: { click: () => { tab = t; render(); } } }, t.toUpperCase())),
    el("span", { className: "grow" }), el("button", { className: "small", on: { click: addDevice() } }, "+ Device"), el("button", { className: "small", on: { click: run(load) } }, "Refresh"));
  show(el("div", { className: "row" }, wsSel), el("div", { className: "row", style: "margin-top:8px" }, pSel), ...ghostBar(), tabs,
    ...(project ? (tab === "tasks" ? tasks() : tab === "boards" ? boards() : media()) : [el("p", { className: "dim" }, "Nothing shared in this workspace yet.")]));
}

// Never switch accounts silently: show whose account the link is for, and refuse if this phone is already signed in.
async function deviceScreen(code) {
  const clear = () => history.replaceState(null, "", "/");
  if (token) {
    clear();
    show(el("h1", {}, "ALREADY SIGNED IN"), el("p", { className: "dim" }, "This phone is already signed in. That device link wasn't used."), el("button", { on: { click: run(start) } }, "OK"));
    return;
  }
  const who = await api("POST", "/v1/devices/claim", { token: code, preview: true });
  show(el("h1", {}, "SIGN IN THIS DEVICE"), el("p", {}, `Sign this device in as ${who.name}?`),
    el("p", { className: "dim" }, "Only continue if you made this link yourself on your other device."),
    el("div", { className: "row" },
      el("button", { className: "primary", on: { click: run(async () => {
        const r = await api("POST", "/v1/devices/claim", { token: code, device_name: /iPad/.test(navigator.userAgent) ? "iPad" : "iPhone" });
        token = r.device.token; localStorage.setItem(KEY, token); clear(); await start();
      }) } }, `Yes, I'm ${who.name}`),
      el("button", { on: { click: () => { clear(); start(); } } }, "Cancel")));
}

function addDevice() {
  return run(async () => {
    const r = await api("POST", "/v1/devices/link");
    const link = `${location.origin}/#dev-${r.token}`;
    const box = el("input", { value: link, readOnly: true, className: "grow", on: { focus: (e) => e.target.select() } });
    show(el("h1", {}, "ADD ANOTHER DEVICE"), el("p", { className: "dim" }, "Open this link on your other device within 15 minutes (it works once). Tailscale must be on there too."),
      el("div", { className: "row" }, box), el("p"), el("button", { on: { click: run(load) } }, "Done"));
    box.focus();
  });
}

async function start() {
  const dev = (location.hash.match(/^#dev-([0-9a-f]{64})$/) || [])[1];
  if (dev) return deviceScreen(dev);
  const code = (location.hash.match(/^#([0-9a-f]{64})$/) || [])[1];
  if (code) return joinScreen(code);
  if (!token) return show(el("h1", {}, "PAPERTRAIL // TEAM"), el("p", { className: "dim" }, "Open the invite link Paper sent you on this phone (with Tailscale on) to join."));
  me = await api("GET", "/v1/me");
  $("who").textContent = me.name;
  if (!me.workspaces.length) return show(el("p", { className: "dim" }, "You're not in any workspace right now."));
  if (!me.workspaces.some((w) => w.id === ws)) ws = me.workspaces[0].id;
  await load();
}

window.addEventListener("hashchange", () => start().catch((e) => say(e.message)));
document.addEventListener("visibilitychange", () => {
  if (!document.hidden && token && ws) load().catch(() => {});
  if (document.hidden) stopWake(); else if (localStorage.getItem("ptcc-ghost-wake") === "1") startWake();
});
setInterval(() => { if (!document.hidden && token && ws) load().catch(() => {}); }, 30000);
start().catch((e) => say(e.message));
