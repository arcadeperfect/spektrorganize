// How much of the picture to render for viewing: one preference, shared by
// the library's full-screen view, Develop and Print, and remembered.
//
//   low     a fitted preview — quick, what you use while working
//   high    enough for a close look — up to 4096 px
//   native  every pixel the file has — slow, and the only honest 1:1

export type Quality = "low" | "high" | "native";
export const QUALITIES: { id: Quality; label: string; hint: string }[] = [
  { id: "low", label: "low", hint: "A fitted preview: quick" },
  { id: "high", label: "high", hint: "Up to 4096 px: enough for a close look" },
  { id: "native", label: "native", hint: "Every pixel the file has: slow, and the only honest 1:1" },
];

const KEY = "spektrorganize.quality";

class QualityStore {
  value = $state<Quality>(load());

  set(q: Quality) {
    this.value = q;
    try {
      localStorage.setItem(KEY, q);
    } catch {
      // Not remembered, still applied.
    }
  }

  /** Pixels on the long edge for a photo whose own long edge is `native` (0 = unknown). */
  px(native: number): number {
    switch (this.value) {
      case "low":
        return 1600;
      case "high":
        return native > 0 ? Math.min(4096, native) : 4096;
      case "native":
        return native > 0 ? Math.min(native, 16384) : 16384;
    }
  }
}

function load(): Quality {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "low" || v === "high" || v === "native") return v;
  } catch {
    // Fall through to the default.
  }
  return "low";
}

export const quality = new QualityStore();
