/* Particle sphere engine: raw WebGL2, one draw call for particles, one for link arcs.
   All motion lives in the vertex shader (rotation of each particle about its own axis by time × speed),
   so the CPU only uploads a few uniforms per frame and projects ~N project nodes for picking.
   Frame-capped, pausable, adaptive; never allocates per frame. */

export type SphereNode = {
  id: string;
  workspace: string;
  color: [number, number, number];
  activityDays: number; // days since last recorded activity
  weight: number; // open tasks, drives core size
  parent?: string; // sub-project: placed as a satellite of its parent node
};
export type SphereLink = { a: string; b: string; n: number };

const FLOATS = 16; // pos3 axis3 color3 size speed kind node phase lift ws

const VS = `#version 300 es
layout(location=0) in vec3 a_pos;
layout(location=1) in vec3 a_axis;
layout(location=2) in vec3 a_color;
layout(location=3) in vec4 a_p; // size, speed, kind, node
layout(location=4) in vec2 a_q; // phase, lift
layout(location=5) in float a_ws; // workspace index (-1 none)
uniform mat3 u_model;
uniform float u_time, u_dist, u_focal, u_aspect, u_dpr, u_hover, u_hoverAge, u_focus, u_ws;
out vec3 v_color;
out float v_alpha;

vec3 rotAxis(vec3 p, vec3 k, float a) {
  float c = cos(a), s = sin(a);
  return p * c + cross(k, p) * s + k * dot(k, p) * (1.0 - c);
}
vec3 slerpLift(vec3 a, vec3 b, float t, float lift) {
  float om = acos(clamp(dot(a, b), -1.0, 1.0));
  vec3 p = om < 0.001 ? a : (sin((1.0 - t) * om) * a + sin(t * om) * b) / sin(om);
  return p * (1.0 + lift * sin(3.14159 * t));
}

void main() {
  float size = a_p.x, speed = a_p.y, kind = a_p.z, node = a_p.w;
  vec3 p;
  float glow = 1.0;
  if (kind > 4.5) {
    // pulse travelling along a link arc: a_pos -> a_axis
    float t = fract(u_time * speed + a_q.x);
    p = slerpLift(a_pos, a_axis, t, a_q.y);
    glow = smoothstep(0.0, 0.15, t) * smoothstep(1.0, 0.85, t);
  } else {
    p = rotAxis(a_pos, a_axis, u_time * speed);
    if (kind < 0.5) p *= 1.0 + 0.012 * sin(u_time * 0.6 + a_q.x * 6.2831); // shell breathing
  }
  bool mine = abs(node - u_hover) < 0.5 && node >= 0.0;
  float ripple = 0.0;
  if (mine && u_hoverAge < 2.0) {
    // expanding ring through the hovered node's cluster
    float r = u_hoverAge * 0.35;
    ripple = exp(-pow((a_q.x - r) * 18.0, 2.0)) * (1.0 - u_hoverAge / 2.0);
  }
  if (kind > 3.5 && kind < 4.5) { // node core pulse; speed = pulse frequency, lift = amplitude
    size *= 1.0 + a_q.y * sin(u_time * speed * 6.2831 + a_q.x * 6.2831);
  }
  vec3 w = u_model * p;
  float depth = u_dist - w.z;
  gl_Position = vec4(w.x * u_focal / u_aspect, w.y * u_focal, 0.0, depth);
  float facing = clamp(0.5 + 0.5 * w.z, 0.0, 1.0); // back of the sphere is dimmer
  bool wsLit = u_ws >= 0.0 && abs(a_ws - u_ws) < 0.5;
  float hoverBoost = mine ? 1.8 : (u_hover >= 0.0 && node >= 0.0 ? 0.55 : 1.0);
  if (wsLit) { hoverBoost = 2.2; size *= 1.3; }
  float focusDim = (u_focus >= 0.0 && node >= 0.0 && abs(node - u_focus) > 0.5) ? 0.4 : 1.0;
  gl_PointSize = max(1.0, size * u_dpr * (1.0 + ripple * 2.5) * (mine ? 1.35 : 1.0) / depth * 3.0);
  v_color = a_color * (1.0 + ripple * 2.0);
  v_alpha = glow * mix(0.28, 1.0, facing) * hoverBoost * focusDim;
}`;

const FS = `#version 300 es
precision mediump float;
in vec3 v_color;
in float v_alpha;
out vec4 o;
void main() {
  vec2 d = gl_PointCoord - 0.5;
  float r = length(d);
  float a = smoothstep(0.5, 0.0, r);
  a *= a;
  vec3 c = v_color * a * v_alpha;
  // Premultiplied, alpha = brightness: particles add light over the page instead of painting dark specks.
  o = vec4(c, clamp(max(c.r, max(c.g, c.b)), 0.0, 1.0));
}`;

const LVS = `#version 300 es
layout(location=0) in vec3 a_pos;
layout(location=1) in float a_t;
uniform mat3 u_model;
uniform float u_dist, u_focal, u_aspect, u_time;
out float v_a;
void main() {
  vec3 w = u_model * a_pos;
  gl_Position = vec4(w.x * u_focal / u_aspect, w.y * u_focal, 0.0, u_dist - w.z);
  v_a = (0.35 + 0.65 * clamp(0.5 + 0.5 * w.z, 0.0, 1.0)) * (0.55 + 0.45 * sin(a_t * 12.0 - u_time * 1.4));
}`;
const LFS = `#version 300 es
precision mediump float;
in float v_a;
uniform vec3 u_lcolor;
out vec4 o;
void main() { vec3 c = u_lcolor * v_a * 0.6; o = vec4(c, max(c.r, max(c.g, c.b))); }`;

// Atmosphere: a full-screen triangle; the fragment shader shades by distance from the sphere's projected rim.
const GVS = `#version 300 es
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)) * 2.0 - 1.0;
  gl_Position = vec4(p, 0.0, 1.0);
}`;
const GFS = `#version 300 es
precision mediump float;
uniform vec2 u_center;
uniform float u_rad, u_mode, u_glow;
out vec4 o;
void main() {
  float d = length(gl_FragCoord.xy - u_center) / u_rad;
  if (u_mode < 0.5) {
    // backing: near-black disc that fades out past the rim (normal blending)
    float a = 0.72 * (1.0 - smoothstep(0.9, 1.45, d));
    o = vec4(vec3(0.004, 0.006, 0.03) * a, a);
  } else {
    float inner = (1.0 - smoothstep(0.0, 1.05, d)) * 0.13 + smoothstep(0.45, 1.0, d) * step(d, 1.0) * 0.16;
    float rim = exp(-pow((d - 0.995) * 11.0, 2.0)) * 0.42;
    float halo = exp(-max(d - 1.0, 0.0) * 5.0) * step(1.0, d) * 0.16;
    vec3 c = vec3(0.16, 0.30, 1.0) * (inner + rim + halo) * u_glow;
    o = vec4(c, max(c.r, max(c.g, c.b)));
  }
}`;

/* --- small math --- */
type V3 = [number, number, number];
const norm = (v: V3): V3 => { const l = Math.hypot(v[0], v[1], v[2]) || 1; return [v[0] / l, v[1] / l, v[2] / l]; };
const cross = (a: V3, b: V3): V3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const dot = (a: V3, b: V3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
function rot(p: V3, k: V3, a: number): V3 {
  const c = Math.cos(a), s = Math.sin(a), kp = cross(k, p), d = dot(k, p) * (1 - c);
  return [p[0] * c + kp[0] * s + k[0] * d, p[1] * c + kp[1] * s + k[1] * d, p[2] * c + kp[2] * s + k[2] * d];
}
function rng(seed: number) {
  let s = seed >>> 0 || 1;
  return () => ((s = (s * 1664525 + 1013904223) >>> 0) / 4294967296);
}
function fib(i: number, n: number): V3 {
  const y = 1 - (2 * (i + 0.5)) / n, r = Math.sqrt(1 - y * y), th = i * 2.399963229728653;
  return [r * Math.cos(th), y, r * Math.sin(th)];
}
function gauss(r: () => number) {
  return Math.sqrt(-2 * Math.log(Math.max(1e-6, r()))) * Math.cos(6.2831853 * r());
}
/** Model matrix (column-major mat3) = rotX(tilt) · rotY(yaw). */
function model(yaw: number, tilt: number): Float32Array {
  const cy = Math.cos(yaw), sy = Math.sin(yaw), cx = Math.cos(tilt), sx = Math.sin(tilt);
  // rows: [cy,0,sy],[sx*sy,cx,-sx*cy],[-cx*sy,sx,cx*cy]
  return new Float32Array([cy, sx * sy, -cx * sy, 0, cx, sx, sy, -sx * cy, cx * cy]);
}
const apply = (m: Float32Array, p: V3): V3 => [
  m[0] * p[0] + m[3] * p[1] + m[6] * p[2],
  m[1] * p[0] + m[4] * p[1] + m[7] * p[2],
  m[2] * p[0] + m[5] * p[1] + m[8] * p[2],
];

/** Node directions: workspaces spread by Fibonacci lattice, their projects ringed around the workspace centre. */
export function layoutNodes(nodes: SphereNode[]): Map<string, V3> {
  const ws = [...new Set(nodes.map((n) => n.workspace))].sort();
  const out = new Map<string, V3>();
  ws.forEach((w, wi) => {
    const c = fib(wi, Math.max(ws.length, 2));
    const all = nodes.filter((n) => n.workspace === w);
    const members = all.filter((n) => !n.parent || !all.some((m) => m.id === n.parent));
    const t1 = norm(cross(c, Math.abs(c[1]) < 0.9 ? [0, 1, 0] : [1, 0, 0]));
    const t2 = cross(c, t1);
    members.forEach((m, i) => {
      if (members.length === 1) return out.set(m.id, c);
      const a = (i / members.length) * Math.PI * 2, r = 0.2 + 0.03 * members.length;
      out.set(m.id, norm([c[0] + (t1[0] * Math.cos(a) + t2[0] * Math.sin(a)) * r, c[1] + (t1[1] * Math.cos(a) + t2[1] * Math.sin(a)) * r, c[2] + (t1[2] * Math.cos(a) + t2[2] * Math.sin(a)) * r]));
    });
    // satellites: tight ring around their parent
    for (const m of members) {
      const kids = all.filter((n) => n.parent === m.id);
      const pd = out.get(m.id)!;
      const k1 = norm(cross(pd, Math.abs(pd[1]) < 0.9 ? [0, 1, 0] : [1, 0, 0])), k2 = cross(pd, k1);
      kids.forEach((k, i) => {
        const a = (i / kids.length) * Math.PI * 2 + 0.6, r = 0.075;
        out.set(k.id, norm([pd[0] + (k1[0] * Math.cos(a) + k2[0] * Math.sin(a)) * r, pd[1] + (k1[1] * Math.cos(a) + k2[1] * Math.sin(a)) * r, pd[2] + (k1[2] * Math.cos(a) + k2[2] * Math.sin(a)) * r]));
      });
    }
  });
  return out;
}

export type Quality = { shell: number; ribbons: number; ribbonPts: number; halo: number; cluster: number };
export function qualityTier(): Quality {
  const cores = navigator.hardwareConcurrency || 4;
  const big = window.screen.width * window.devicePixelRatio > 2600;
  if (cores >= 12) return { shell: 9000, ribbons: 6, ribbonPts: 1300, halo: 500, cluster: 110 };
  if (cores >= 6 && !big) return { shell: 7000, ribbons: 5, ribbonPts: 1000, halo: 400, cluster: 90 };
  return { shell: 6000, ribbons: 4, ribbonPts: 900, halo: 300, cluster: 80 };
}

const SHELL: V3 = [0.22, 0.45, 1.0];
const SHELL2: V3 = [0.45, 0.78, 1.0];
const RIBBON: V3 = [0.72, 0.62, 1.0];
const RIBBON_HOT: V3 = [1.0, 0.72, 0.95];

/** Every engine ever created that hasn't been disposed. A new engine disposes the rest, so no orphaned
 *  render loop can keep drawing into a detached canvas. */
const LIVE = new Set<SphereEngine>();
export const liveEngines = () => LIVE.size;

export class SphereEngine {
  private gl: WebGL2RenderingContext;
  private prog: WebGLProgram;
  private lprog: WebGLProgram;
  private gprog: WebGLProgram;
  private gu: Record<string, WebGLUniformLocation | null> = {};
  private vao: WebGLVertexArrayObject;
  private lvao: WebGLVertexArrayObject;
  private count = 0;
  private trimmable = 0; // shell particles are last in the buffer and can be dropped under load
  private drawCount = 0;
  private lineCount = 0;
  private u: Record<string, WebGLUniformLocation | null> = {};
  private lu: Record<string, WebGLUniformLocation | null> = {};
  private dirs = new Map<string, V3>();
  private nodeIndex = new Map<string, number>();
  private raf = 0;
  private last = 0;
  private time = 0;
  private slow: number[] = [];
  yaw = 0.6;
  tilt = 0.32;
  dist = 3.4;
  hover = -1;
  wsHover = -1;
  private wsIndex = new Map<string, number>();
  focus = -1;
  hoverAt = 0;
  running = false;
  fps = 24;
  reduced = false;
  onFrame?: () => void;

  constructor(private canvas: HTMLCanvasElement, nodes: SphereNode[], links: SphereLink[], q: Quality) {
    for (const old of [...LIVE]) old.dispose();
    LIVE.add(this);
    const gl = canvas.getContext("webgl2", { antialias: false, alpha: true, premultipliedAlpha: true, powerPreference: "low-power" });
    if (!gl) throw new Error("WebGL2 unavailable");
    this.gl = gl;
    this.prog = program(gl, VS, FS);
    this.lprog = program(gl, LVS, LFS);
    this.gprog = program(gl, GVS, GFS);
    for (const n of ["u_center", "u_rad", "u_mode", "u_glow"]) this.gu[n] = gl.getUniformLocation(this.gprog, n);
    for (const n of ["u_model", "u_time", "u_dist", "u_focal", "u_aspect", "u_dpr", "u_hover", "u_hoverAge", "u_focus", "u_ws"]) this.u[n] = gl.getUniformLocation(this.prog, n);
    for (const n of ["u_model", "u_time", "u_dist", "u_focal", "u_aspect", "u_lcolor"]) this.lu[n] = gl.getUniformLocation(this.lprog, n);
    this.dirs = layoutNodes(nodes);
    nodes.forEach((n, i) => this.nodeIndex.set(n.id, i));
    [...new Set(nodes.map((n) => n.workspace))].sort().forEach((w, i) => this.wsIndex.set(w, i));
    const { data, count, trimmable } = buildParticles(nodes, links, this.dirs, q);
    this.count = this.drawCount = count;
    this.trimmable = trimmable;
    this.vao = gl.createVertexArray()!;
    gl.bindVertexArray(this.vao);
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
    const S = FLOATS * 4;
    const attr = (loc: number, size: number, off: number) => { gl.enableVertexAttribArray(loc); gl.vertexAttribPointer(loc, size, gl.FLOAT, false, S, off * 4); };
    attr(0, 3, 0); attr(1, 3, 3); attr(2, 3, 6); attr(3, 4, 9); attr(4, 2, 13); attr(5, 1, 15);
    const lines = buildLines(links, this.dirs);
    this.lineCount = lines.length / 4;
    this.lvao = gl.createVertexArray()!;
    gl.bindVertexArray(this.lvao);
    const lb = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, lb);
    gl.bufferData(gl.ARRAY_BUFFER, lines, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0); gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 16, 0);
    gl.enableVertexAttribArray(1); gl.vertexAttribPointer(1, 1, gl.FLOAT, false, 16, 12);
    gl.bindVertexArray(null);
    gl.disable(gl.DEPTH_TEST);
    gl.enable(gl.BLEND);
    gl.blendFuncSeparate(gl.ONE, gl.ONE, gl.ONE, gl.ONE); // additive light, coverage accumulates in alpha
  }

  get particleCount() { return this.drawCount; }
  index(id: string) { return this.nodeIndex.get(id) ?? -1; }
  wsIdx(ws: string | null) { return ws ? this.wsIndex.get(ws) ?? -1 : -1; }

  resize() {
    const dpr = 1; // fill-rate is the main cost on integrated GPUs; particles are soft points, 1x looks the same
    const w = Math.max(1, Math.round(this.canvas.clientWidth * dpr)), h = Math.max(1, Math.round(this.canvas.clientHeight * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) { this.canvas.width = w; this.canvas.height = h; }
    this.gl.viewport(0, 0, w, h);
  }

  /** Screen positions (CSS px) of project nodes for picking/labels; z>0 = front hemisphere. */
  project(): { id: string; x: number; y: number; z: number }[] {
    const m = model(this.yaw, this.tilt);
    const W = this.canvas.clientWidth, H = this.canvas.clientHeight, f = this.focal(), aspect = W / H;
    const out: { id: string; x: number; y: number; z: number }[] = [];
    for (const [id, d] of this.dirs) {
      const w = apply(m, d);
      const depth = this.dist - w[2];
      out.push({ id, x: (w[0] * f / aspect / depth * 0.5 + 0.5) * W, y: (0.5 - w[1] * f / depth * 0.5) * H, z: w[2] });
    }
    return out;
  }
  dir(id: string) { return this.dirs.get(id); }
  private focal() { return 2.55; }

  draw() {
    const gl = this.gl;
    this.resize();
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    const m = model(this.yaw, this.tilt);
    const aspect = this.canvas.clientWidth / Math.max(1, this.canvas.clientHeight);
    // atmosphere: projected silhouette radius of the unit sphere = f / sqrt(d^2 - 1) in NDC
    const radPx = (this.focal() / Math.sqrt(Math.max(1.01, this.dist * this.dist - 1))) * 0.5 * this.canvas.height;
    gl.useProgram(this.gprog);
    gl.bindVertexArray(null);
    gl.uniform2f(this.gu.u_center, this.canvas.width / 2, this.canvas.height / 2);
    gl.uniform1f(this.gu.u_rad, radPx);
    gl.uniform1f(this.gu.u_glow, 1.0 + 0.05 * Math.sin(this.time * 0.5));
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.uniform1f(this.gu.u_mode, 0);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    gl.blendFuncSeparate(gl.ONE, gl.ONE, gl.ONE, gl.ONE);
    gl.uniform1f(this.gu.u_mode, 1);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    gl.useProgram(this.lprog);
    gl.uniformMatrix3fv(this.lu.u_model, false, m);
    gl.uniform1f(this.lu.u_dist, this.dist); gl.uniform1f(this.lu.u_focal, this.focal()); gl.uniform1f(this.lu.u_aspect, aspect);
    gl.uniform1f(this.lu.u_time, this.time); gl.uniform3f(this.lu.u_lcolor, 0.7, 0.6, 1.0);
    gl.bindVertexArray(this.lvao);
    gl.drawArrays(gl.LINES, 0, this.lineCount);
    gl.useProgram(this.prog);
    gl.uniformMatrix3fv(this.u.u_model, false, m);
    gl.uniform1f(this.u.u_time, this.time); gl.uniform1f(this.u.u_dist, this.dist); gl.uniform1f(this.u.u_focal, this.focal());
    gl.uniform1f(this.u.u_aspect, aspect); gl.uniform1f(this.u.u_dpr, 1);
    gl.uniform1f(this.u.u_hover, this.hover); gl.uniform1f(this.u.u_hoverAge, (performance.now() - this.hoverAt) / 1000);
    gl.uniform1f(this.u.u_focus, this.focus);
    gl.uniform1f(this.u.u_ws, this.wsHover);
    gl.bindVertexArray(this.vao);
    gl.drawArrays(gl.POINTS, 0, this.drawCount);
    gl.bindVertexArray(null);
    this.onFrame?.();
  }

  /** Capped loop. Time only advances while running and not in reduced-motion mode. */
  start() {
    if (this.running) return;
    this.running = true;
    this.last = performance.now();
    const tick = (now: number) => {
      if (!this.running) return;
      this.raf = requestAnimationFrame(tick);
      const dt = now - this.last;
      if (dt < 1000 / this.fps - 2) return;
      this.last = now;
      if (!this.reduced) {
        this.time += Math.min(dt, 100) / 1000;
        this.yaw += (Math.min(dt, 100) / 1000) * 0.045; // slow revolve
      }
      this.adapt(dt);
      this.draw();
    };
    this.raf = requestAnimationFrame(tick);
  }
  stop() {
    this.running = false;
    cancelAnimationFrame(this.raf);
  }
  /** If frames consistently arrive late, drop shell particles (never nodes, links or ribbons). */
  private adapt(dt: number) {
    this.slow.push(dt);
    if (this.slow.length < 40) return;
    const avg = this.slow.reduce((a, b) => a + b, 0) / this.slow.length;
    this.slow.length = 0;
    const budget = 1000 / this.fps;
    const floor = this.count - this.trimmable;
    if (avg > budget * 1.45 && this.drawCount > floor + 1000) this.drawCount = Math.max(floor, Math.round(this.drawCount * 0.75));
    else if (avg < budget * 1.1 && this.drawCount < this.count) this.drawCount = Math.min(this.count, this.drawCount + 1500);
  }
  rendererInfo(): string {
    const ext = this.gl.getExtension("WEBGL_debug_renderer_info");
    return ext ? String(this.gl.getParameter(ext.UNMASKED_RENDERER_WEBGL)) : String(this.gl.getParameter(this.gl.RENDERER));
  }
  dispose() {
    LIVE.delete(this);
    this.stop();
    this.gl.getExtension("WEBGL_lose_context")?.loseContext();
  }
}

function program(gl: WebGL2RenderingContext, vs: string, fs: string) {
  const sh = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) || "shader");
    return s;
  };
  const p = gl.createProgram()!;
  gl.attachShader(p, sh(gl.VERTEX_SHADER, vs));
  gl.attachShader(p, sh(gl.FRAGMENT_SHADER, fs));
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) || "link");
  return p;
}

/** Order: node cores, node clusters, pulses, trails, ribbons, halo, then shell (trimmable). */
export function buildParticles(nodes: SphereNode[], links: SphereLink[], dirs: Map<string, V3>, q: Quality) {
  const r = rng(7);
  const rows: number[] = [];
  const wsIds = [...new Set(nodes.map((n) => n.workspace))].sort();
  let curWs = -1; // workspace of the node being emitted (-1 for ambient particles)
  const push = (p: V3, axis: V3, c: V3, size: number, speed: number, kind: number, node: number, phase: number, lift: number) =>
    rows.push(p[0], p[1], p[2], axis[0], axis[1], axis[2], c[0], c[1], c[2], size, speed, kind, node, phase, lift, curWs);
  const Y: V3 = [0, 1, 0];
  nodes.forEach((n, i) => {
    const d = dirs.get(n.id)!;
    const live = n.activityDays < 1 ? 1 : n.activityDays < 7 ? 0.8 : n.activityDays < 30 ? 0.55 : 0.3;
    const freq = n.activityDays < 1 ? 0.9 : n.activityDays < 7 ? 0.5 : n.activityDays < 30 ? 0.25 : 0.0;
    curWs = wsIds.indexOf(n.workspace);
    const live2 = Math.max(live, 0.55); // every project stays clearly visible; activity only modulates
    const c: V3 = [n.color[0] * live2, n.color[1] * live2, n.color[2] * live2];
    // soft wide halo, then a bright core (projects must read clearly on the map)
    push(d, Y, [c[0] * 0.55, c[1] * 0.55, c[2] * 0.7], 30 + Math.min(14, Math.sqrt(n.weight) * 3), freq, 4, i, r(), freq ? 0.12 : 0);
    push(d, Y, [Math.min(1.6, c[0] * 2.4 + 0.3), Math.min(1.6, c[1] * 2.4 + 0.3), Math.min(1.6, c[2] * 2.4 + 0.3)], 13 + Math.min(8, Math.sqrt(n.weight) * 2.4), freq, 4, i, r(), freq ? 0.22 : 0);
    // cluster: gaussian cloud hugging the surface around the node; phase = distance (for the hover ripple)
    const t1 = norm(cross(d, Math.abs(d[1]) < 0.9 ? Y : [1, 0, 0])), t2 = cross(d, t1);
    for (let k = 0; k < q.cluster; k++) {
      const a = gauss(r) * 0.07, b = gauss(r) * 0.07;
      const p = norm([d[0] + t1[0] * a + t2[0] * b, d[1] + t1[1] * a + t2[1] * b, d[2] + t1[2] * a + t2[2] * b]);
      const h = 1 + Math.abs(gauss(r)) * 0.012;
      const mix = 0.55 + r() * 0.45;
      push([p[0] * h, p[1] * h, p[2] * h], d, [c[0] * mix + 0.15, c[1] * mix + 0.15, c[2] * mix + 0.25], 2.2 + r() * 1.6, (r() - 0.5) * 0.08, 3, i, Math.hypot(a, b), 0);
    }
    // recent activity: a small orbiting energy trail around the node
    if (n.activityDays < 2) {
      for (let k = 0; k < 90; k++) {
        const ang = (k / 90) * Math.PI * 1.4, rr = 0.11;
        const p = norm([d[0] + (t1[0] * Math.cos(ang) + t2[0] * Math.sin(ang)) * rr, d[1] + (t1[1] * Math.cos(ang) + t2[1] * Math.sin(ang)) * rr, d[2] + (t1[2] * Math.cos(ang) + t2[2] * Math.sin(ang)) * rr]);
        const fade = k / 90;
        push([p[0] * 1.03, p[1] * 1.03, p[2] * 1.03], d, [RIBBON_HOT[0] * fade, RIBBON_HOT[1] * fade, RIBBON_HOT[2] * fade], 2.4, 0.9, 2, i, ang / 4, 0);
      }
    }
  });
  curWs = -1;
  // pulses along verified links
  for (const l of links) {
    const a = dirs.get(l.a), b = dirs.get(l.b);
    if (!a || !b) continue;
    const n = Math.min(3, 1 + Math.floor(Math.log2(1 + l.n)));
    for (let k = 0; k < n; k++) push(a, b, [1.0, 0.9, 1.0], 7, 0.07 + r() * 0.04, 5, -1, r(), 0.18);
    // glowing arc: dense soft points along the same lifted great-circle (static; the pulses travel over it)
    const om = Math.acos(Math.max(-1, Math.min(1, dot(a, b))));
    const steps = 64;
    for (let k = 0; k <= steps; k++) {
      const t = k / steps, sn = Math.sin(om) || 1;
      const wa = om < 1e-3 ? 1 - t : Math.sin((1 - t) * om) / sn, wb = om < 1e-3 ? t : Math.sin(t * om) / sn, lift = 1 + 0.18 * Math.sin(Math.PI * t);
      const q: V3 = [(a[0] * wa + b[0] * wb) * lift, (a[1] * wa + b[1] * wb) * lift, (a[2] * wa + b[2] * wb) * lift];
      const g = 0.65 + 0.3 * Math.min(1, l.n / 6);
      push(q, Y, [0.62 * g, 0.5 * g, 1.0 * g], 4.2, 0, 2, -1, t, 0);
    }
  }
  // ribbons: wobbling bands around random axes; rotating about their axis makes them flow along themselves
  for (let k = 0; k < q.ribbons; k++) {
    const axis = norm([gauss(r), gauss(r), gauss(r)]);
    const u1 = norm(cross(axis, Math.abs(axis[1]) < 0.9 ? Y : [1, 0, 0])), u2 = cross(axis, u1);
    const lat = (r() - 0.5) * 1.1, span = 1.2 + r() * 1.6, wob = 0.18 + r() * 0.2, freq = 2 + Math.floor(r() * 3), width = 0.18 + r() * 0.14;
    const speed = (0.06 + r() * 0.08) * (r() < 0.5 ? -1 : 1);
    for (let j = 0; j < q.ribbonPts; j++) {
      const s = r() * span, across = gauss(r) * width * 0.35;
      const la = lat + wob * Math.sin(s * freq) + across;
      const p = norm([
        u1[0] * Math.cos(s) * Math.cos(la) + u2[0] * Math.sin(s) * Math.cos(la) + axis[0] * Math.sin(la),
        u1[1] * Math.cos(s) * Math.cos(la) + u2[1] * Math.sin(s) * Math.cos(la) + axis[1] * Math.sin(la),
        u1[2] * Math.cos(s) * Math.cos(la) + u2[2] * Math.sin(s) * Math.cos(la) + axis[2] * Math.sin(la),
      ]);
      const h = 1.015 + Math.abs(across) * 0.35 + 0.035 * Math.sin(s * freq * 1.7 + across * 9);
      const edge = Math.pow(Math.min(1, Math.min(s, span - s) / 0.35), 1.5); // taper the ends
      const hot = Math.max(0, 1 - Math.abs(across) / (width * 0.5));
      const k = hot * 0.85;
      const c: V3 = [RIBBON[0] + (RIBBON_HOT[0] - RIBBON[0]) * k, RIBBON[1] + (RIBBON_HOT[1] - RIBBON[1]) * k, RIBBON[2] + (RIBBON_HOT[2] - RIBBON[2]) * k];
      push([p[0] * h, p[1] * h, p[2] * h], axis, [c[0] * edge * 1.15, c[1] * edge * 1.1, c[2] * edge * 1.15], 1.8 + r() * 1.5, speed, 1, -1, r(), 0);
    }
  }
  // halo: sparse stray particles drifting just outside the shell
  for (let k = 0; k < q.halo; k++) {
    const d = norm([gauss(r), gauss(r), gauss(r)]), h = 1.05 + Math.pow(r(), 2.5) * 0.45;
    push([d[0] * h, d[1] * h, d[2] * h], norm([gauss(r), gauss(r), gauss(r)]), [0.35 * (1.6 - h), 0.55 * (1.6 - h), 1.0 * (1.6 - h)], 1.4 + r() * 1.4, 0.02 + r() * 0.04, 2, -1, r(), 0);
  }
  const before = rows.length / FLOATS;
  // shell: faint, near-uniform surface dust (additive blending makes the rim glow on its own)
  for (let k = 0; k < q.shell; k++) {
    const d = fib(k, q.shell);
    const j = norm([d[0] + gauss(r) * 0.012, d[1] + gauss(r) * 0.012, d[2] + gauss(r) * 0.012]);
    const t = r();
    const c: V3 = [SHELL[0] + (SHELL2[0] - SHELL[0]) * t, SHELL[1] + (SHELL2[1] - SHELL[1]) * t, SHELL[2]];
    const b = 0.45 + r() * 0.45;
    push(j, norm([gauss(r), gauss(r), gauss(r)]), [c[0] * b, c[1] * b, c[2] * b], 1.6 + r() * 1.3, 0.01 + r() * 0.015, 0, -1, r(), 0);
  }
  const count = rows.length / FLOATS;
  return { data: new Float32Array(rows), count, trimmable: count - before };
}

function buildLines(links: SphereLink[], dirs: Map<string, V3>) {
  const out: number[] = [];
  const SEG = 28;
  for (const l of links) {
    const a = dirs.get(l.a), b = dirs.get(l.b);
    if (!a || !b) continue;
    const om = Math.acos(Math.max(-1, Math.min(1, dot(a, b))));
    const pt = (t: number): V3 => {
      if (om < 1e-3) return a;
      const s = Math.sin(om), wa = Math.sin((1 - t) * om) / s, wb = Math.sin(t * om) / s, lift = 1 + 0.18 * Math.sin(Math.PI * t);
      return [(a[0] * wa + b[0] * wb) * lift, (a[1] * wa + b[1] * wb) * lift, (a[2] * wa + b[2] * wb) * lift];
    };
    for (let i = 0; i < SEG; i++) {
      const p = pt(i / SEG), q = pt((i + 1) / SEG);
      out.push(p[0], p[1], p[2], i / SEG, q[0], q[1], q[2], (i + 1) / SEG);
    }
  }
  return new Float32Array(out);
}

/** For the click transition: yaw/tilt that bring a direction to face the camera. */
export function faceAngles(d: V3): { yaw: number; tilt: number } {
  const yaw = Math.atan2(-d[0], d[2]);
  const r = rot(d, [0, 1, 0], -yaw); // after yaw the node lies in the y-z plane
  const tilt = Math.atan2(r[1], r[2]);
  return { yaw, tilt };
}
