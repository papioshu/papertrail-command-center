import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";
import { PHONE } from "./api";

document.body.classList.toggle("phone", PHONE);

createRoot(document.getElementById("root")!).render(<App />);
