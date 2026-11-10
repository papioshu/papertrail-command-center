// Boards on iPhone/iPad (D050): /board/#<workspace>/<board>, opened from the phone page's BOARDS tab. Apple Pencil
// works through Excalidraw's pointer events (pen draws; fingers pan/zoom with "pen mode" on).
import { useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import BoardCanvas from "../views/BoardCanvas";
import { pull, records, type Row } from "./hubApi";
import "../styles.css";

const [ws, id] = location.hash.slice(1).split("/");
const back = () => { location.href = "/"; };

function Page() {
  const [rows, setRows] = useState<Row[] | null>(null);
  const [err, setErr] = useState("");
  const reload = useCallback(() => { pull(ws).then(() => setRows(records()), (e) => setErr(String(e.message ?? e))); }, []);
  useEffect(reload, [reload]);
  if (!localStorage.getItem("ptcc-team-token")) return <p className="dim" style={{ padding: 20 }}>Open the team page first (your invite link) to sign in on this device.</p>;
  if (!rows) return <p className="dim" style={{ padding: 20 }}>{err || "Loading board…"}</p>;
  const board = rows.find((r) => r.id === id && r.kind === "board");
  if (!board) return <div style={{ padding: 20 }}><p>{err || "That board isn't there any more."}</p><button className="btn" onClick={back}>← Team</button></div>;
  return <BoardCanvas ws={ws} board={board} b={{ records: rows }} canDraw onBack={back} onErr={setErr} reload={reload} />;
}

document.body.classList.add("web-board");
createRoot(document.getElementById("root")!).render(<Page />);
