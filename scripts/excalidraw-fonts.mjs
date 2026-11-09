// Boards (D044): Excalidraw's fonts ship inside the app (it would fetch them from a CDN otherwise, which the CSP
// blocks and offline PCs can't reach). Xiaolai, the 13 MB CJK font, is left out; CJK text falls back to system fonts.
import { cpSync, existsSync, readdirSync, rmSync } from "node:fs";

const from = "node_modules/@excalidraw/excalidraw/dist/prod/fonts";
const to = "public/excalidraw-assets/fonts";
rmSync(to, { recursive: true, force: true });
for (const family of readdirSync(from)) {
  if (family !== "Xiaolai") cpSync(`${from}/${family}`, `${to}/${family}`, { recursive: true });
}
if (!existsSync(`${to}/Excalifont`)) throw new Error("Excalidraw fonts missing: run npm install");
