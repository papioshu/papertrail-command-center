/** Just enough markdown for agent replies, as data; the view turns it into React elements (never raw HTML). */
export type MdBlock = { t: "code"; lang: string; body: string } | { t: "h"; level: number; text: string }
  | { t: "li"; ordered: boolean; text: string } | { t: "p"; text: string };
export type Inline = { t: "text" | "code" | "bold"; s: string };

export function mdBlocks(text: string): MdBlock[] {
  const out: MdBlock[] = [];
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  let para: string[] = [];
  const flush = () => { if (para.length) out.push({ t: "p", text: para.join("\n") }); para = []; };
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    const fence = l.match(/^\s*```\s*([\w+-]*)/);
    if (fence) {
      flush();
      const body: string[] = [];
      while (++i < lines.length && !/^\s*```/.test(lines[i])) body.push(lines[i]);
      out.push({ t: "code", lang: fence[1], body: body.join("\n") });
      continue;
    }
    const h = l.match(/^(#{1,3})\s+(.*)$/);
    const li = l.match(/^\s*(?:([-*])|(\d+)\.)\s+(.*)$/);
    if (h) { flush(); out.push({ t: "h", level: h[1].length, text: h[2] }); }
    else if (li) { flush(); out.push({ t: "li", ordered: !!li[2], text: li[3] }); }
    else if (!l.trim()) flush();
    else para.push(l);
  }
  flush();
  return out;
}

export function inline(s: string): Inline[] {
  const out: Inline[] = [];
  const re = /`([^`]+)`|\*\*([^*]+)\*\*/g;
  let last = 0;
  for (let m; (m = re.exec(s)); ) {
    if (m.index > last) out.push({ t: "text", s: s.slice(last, m.index) });
    out.push(m[1] !== undefined ? { t: "code", s: m[1] } : { t: "bold", s: m[2] });
    last = re.lastIndex;
  }
  if (last < s.length) out.push({ t: "text", s: s.slice(last) });
  return out;
}
