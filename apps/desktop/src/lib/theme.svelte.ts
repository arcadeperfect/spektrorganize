// UI colours. The palette lives in CSS custom properties on :root; this store
// writes them and remembers the choice per machine (localStorage).

const KEY = "spektrorganize.theme";

export interface Theme {
  accent: string;
  /** Lighter accent for hover / secondary marks. */
  accent2: string;
  /** Text drawn on an accent-filled control. */
  onAccent: string;
  /** Window background; the two panel tones are derived from it. */
  bg: string;
}

export const PRESETS: { name: string; theme: Theme }[] = [
  { name: "Teal", theme: { accent: "#29a89c", accent2: "#4fd1c5", onAccent: "#06201d", bg: "#161514" } },
  { name: "Amber", theme: { accent: "#e88434", accent2: "#f0a45c", onAccent: "#1a120a", bg: "#161514" } },
  { name: "Ink", theme: { accent: "#5b8def", accent2: "#87aef6", onAccent: "#05102a", bg: "#131519" } },
  { name: "Moss", theme: { accent: "#6fae4a", accent2: "#93c974", onAccent: "#0d1806", bg: "#151714" } },
  { name: "Plum", theme: { accent: "#a76ec9", accent2: "#c495e0", onAccent: "#1b0a24", bg: "#17141a" } },
];

export const DEFAULT_THEME = PRESETS[0].theme;

/** Mix a colour toward white by `amount` (0–1), for the panel tones. */
function lighten(hex: string, amount: number): string {
  const n = parseInt(hex.slice(1), 16);
  const ch = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => Math.round(v + (255 - v) * amount));
  return `#${ch.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}

class ThemeStore {
  current = $state<Theme>({ ...DEFAULT_THEME });

  load() {
    try {
      const raw = localStorage.getItem(KEY);
      if (raw) this.current = { ...DEFAULT_THEME, ...JSON.parse(raw) };
    } catch {
      // A blocked or empty store just means the default palette.
    }
    this.apply();
  }

  set(theme: Partial<Theme>) {
    this.current = { ...this.current, ...theme };
    try {
      localStorage.setItem(KEY, JSON.stringify(this.current));
    } catch {
      // Not fatal: the colours still apply for this session.
    }
    this.apply();
  }

  reset() {
    this.set({ ...DEFAULT_THEME });
  }

  private apply() {
    const t = this.current;
    const r = document.documentElement.style;
    r.setProperty("--accent", t.accent);
    r.setProperty("--accent-2", t.accent2);
    r.setProperty("--on-accent", t.onAccent);
    r.setProperty("--bg", t.bg);
    r.setProperty("--bg-2", lighten(t.bg, 0.04));
    r.setProperty("--bg-3", lighten(t.bg, 0.08));
    r.setProperty("--line", lighten(t.bg, 0.16));
  }
}

export const theme = new ThemeStore();
