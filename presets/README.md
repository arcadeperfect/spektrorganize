# Presets

A preset is a JSON file: a film stock, a paper, and any spektrafilm parameter overrides.
Everything under `params` is optional; omitted values use the engine defaults.

```json
{
  "name": "Portra 400 on Portra Endura",
  "film": "kodak_portra_400",
  "print": "kodak_portra_endura",
  "params": { ... }
}
```

Files here ship with the app. Your own go in `~/.config/spektrorganize/presets/` and win on
name clashes. Point `preset` in the config at a file name (`portra.json`) or an absolute path.

Try a preset on one RAW without importing anything:

```bash
spektro try DSCF0001.RAF --preset portra.json --jpeg out.jpg
```

## Stocks

`spektro profiles` lists them. Films: `kodak_portra_160/400/800`, `kodak_portra_800_push1/push2`,
`kodak_ektar_100`, `kodak_gold_200`, `kodak_ultramax_400`, `kodak_vision3_50d/250d/200t/500t`,
`kodak_ektachrome_100`, `kodak_kodachrome_64`, `kodak_trix`, `kodak_doublex`, `fujifilm_c200`,
`fujifilm_pro_400h`, `fujifilm_xtra_400`, `fujifilm_provia_100f`, `fujifilm_velvia_100`.
Papers: `kodak_portra_endura`, `kodak_endura_premier`, `kodak_supra_endura`, `kodak_ultra_endura`,
`kodak_ektacolor_edge`, `fujifilm_crystal_archive_typeii`, and the cine print stocks
`kodak_2383`, `kodak_2393`, `kodak_2302`.

Slide films (Provia, Velvia, Ektachrome, Kodachrome) are positives: set
`"io": { "scan_film": true }` to scan the film itself and skip the print.

## The knobs that matter

```json
"params": {
  "camera": {
    "auto_exposure": true,              // per-image exposure normalisation (center_weighted)
    "exposure_compensation_ev": 0.0,    // stops, on top of auto exposure
    "film_format_mm": 35.0              // 35, 60 (645/6x6), 90... scales grain and halation
  },
  "enlarger": {
    "print_exposure": 1.0,              // >1 darker print, <1 lighter
    "y_filter_shift": 0.0,              // + warmer / - cooler   (enlarger yellow filter, cc steps)
    "m_filter_shift": 0.0,              // + magenta / - green   (enlarger magenta filter, cc steps)
    "preflash_exposure": 0.0            // 0.05-0.2 lifts and softens highlights
  },
  "film_render": {
    "grain": { "active": true, "agx_particle_area_um2": 1.2, "blur": 0.65 },
    "halation": { "active": true, "halation_amount": 1.0, "scatter_amount": 1.0 },
    "dir_couplers": { "active": true, "amount": 1.0 }    // saturation / edge contrast from couplers
  },
  "scanner": {
    "unsharp_mask": [0.7, 0.7]          // [sigma, amount]; [0, 0] = no sharpening
  },
  "settings": {
    "use_enlarger_lut": true,           // keep on; big speedup, same result
    "use_scanner_lut": true
  }
}
```

(Comments are not valid JSON; strip them.) `film_format_mm` is the physical negative width the
image is mapped onto, so a 60 mm setting gives finer grain than 35 at the same output size.

## Finding values

The fastest loop is `spektro try` on one or two representative frames, editing the JSON between
runs. For interactive tuning build the spektrafilm-rs GUI from the same commit this app pins
(`9dd59b0`) and copy the values into a preset; its parameter names are identical.
