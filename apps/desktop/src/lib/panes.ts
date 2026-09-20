// Remembered pane widths. Splitters own nothing: the view holds the width and
// asks here to load and save it, so a pane keeps its size across sessions.

const KEY = "spektrorganize.panes";

function all(): Record<string, number> {
  try {
    const raw = localStorage.getItem(KEY);
    return raw ? (JSON.parse(raw) as Record<string, number>) : {};
  } catch {
    // A blocked or corrupt store just means the default widths.
    return {};
  }
}

export function paneWidth(name: string, fallback: number): number {
  const w = all()[name];
  return typeof w === "number" && Number.isFinite(w) ? w : fallback;
}

export function savePaneWidth(name: string, width: number) {
  try {
    localStorage.setItem(KEY, JSON.stringify({ ...all(), [name]: Math.round(width) }));
  } catch {
    // Not fatal: the width still applies for this session.
  }
}
