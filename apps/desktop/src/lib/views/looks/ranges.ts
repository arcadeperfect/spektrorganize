// Sensible slider ranges for parameters the Pro panel discovers by walking the
// model. Keyed by the last part of the path, then by pattern; anything unknown
// gets a range built around its stock value. Only presentation — nothing here
// changes what a value means.

export interface Range {
  min: number;
  max: number;
  step: number;
}

const EXACT: Record<string, Range> = {
  exposure_compensation_ev: { min: -4, max: 4, step: 0.05 },
  print_exposure: { min: 0.1, max: 4, step: 0.01 },
  gamma_factor: { min: 0.25, max: 4, step: 0.01 },
  film_format_mm: { min: 8, max: 130, step: 1 },
  lens_blur_um: { min: 0, max: 30, step: 0.1 },
  lens_blur: { min: 0, max: 3, step: 0.01 },
  white_level: { min: 0.5, max: 1, step: 0.005 },
  black_level: { min: 0, max: 0.2, step: 0.001 },
  base_percentile: { min: 90, max: 100, step: 0.1 },
  percent: { min: 0, max: 0.2, step: 0.001 },
  roughness: { min: 0, max: 1, step: 0.01 },
  tilt: { min: -1, max: 1, step: 0.01 },
  halo_warmth: { min: -1, max: 1, step: 0.01 },
  spectral_gaussian_blur: { min: 0, max: 10, step: 0.1 },
  upscale_factor: { min: 1, max: 4, step: 0.1 },
};

const PATTERN: [RegExp, Range][] = [
  [/_ev$/, { min: -4, max: 4, step: 0.05 }],
  [/(shift|neutral)$/, { min: -100, max: 100, step: 0.5 }],
  [/_um$/, { min: 0, max: 300, step: 0.5 }],
  [/(uniformity|density_min)/, { min: 0, max: 1, step: 0.005 }],
  [/gamma|^k_|_k_/, { min: 0, max: 1, step: 0.005 }],
  [/(weight|decay|range)$/, { min: 0, max: 1, step: 0.005 }],
  [/(amount|strength|scale|intensity|size|granularity|sigma)/, { min: 0, max: 3, step: 0.01 }],
  [/blur/, { min: 0, max: 5, step: 0.01 }],
];

/** The range to offer for `path` given its stock value. */
export function rangeFor(path: string, stock: number | undefined): Range {
  const leaf = path.split(".").pop() ?? path;
  if (EXACT[leaf]) return EXACT[leaf];
  for (const [re, r] of PATTERN) if (re.test(leaf)) return widen(r, stock);
  // Unknown: three times the stock value, or ±1 around zero.
  const s = Math.abs(stock ?? 0);
  if (s === 0) return { min: -1, max: 1, step: 0.01 };
  return { min: s < 0 ? -3 * s : 0, max: 3 * s, step: s / 100 };
}

/** A stock value outside the pattern's range stretches it, so the slider always reaches it. */
function widen(r: Range, stock: number | undefined): Range {
  if (stock === undefined) return r;
  return { min: Math.min(r.min, stock), max: Math.max(r.max, stock * 1.5), step: r.step };
}

/** Parameters that are not for a look editor at all. */
export const HIDDEN = new Set(["debug", "taps", "workflow.route", "io.input_color_space", "io.input_cctf_decoding", "settings.preview_max_size", "settings.lut_resolution", "settings.convert_lut_resolution"]);

/** Turn `halation_first_sigma_um` into `Halation first sigma (µm)`. */
export function humanize(key: string): string {
  let s = key.replace(/_um2$/, " (µm²)").replace(/_um$/, " (µm)").replace(/_ev$/, " (EV)").replace(/_mm$/, " (mm)").replace(/_rgb$/, " RGB");
  s = s.replace(/_/g, " ");
  return s.charAt(0).toUpperCase() + s.slice(1);
}
