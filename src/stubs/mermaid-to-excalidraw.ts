// Boards (D044): Excalidraw's "Mermaid to Excalidraw" converter pulls ~6 MB of diagram libraries into the installer.
// Boards don't offer it; this stub keeps the app small and fails that one dialog politely if someone opens it.
export const parseMermaidToExcalidraw = async () => {
  throw new Error("Mermaid diagrams aren't available in Command Center boards.");
};
