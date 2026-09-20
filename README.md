# spektrorganize

A photo catalog: import camera cards into a token-templated archive, or index folders that are
already organised (in place, never moved), and browse everything as one library with keywords,
AI labels and fast thumbnails. RAWs are "printed" through a
[spektrafilm](https://github.com/andreavolpato/spektrafilm) film preset to EXR (for grading)
and/or JPEG (for looking at), on import or later. No Python anywhere.

- Decode: LibRaw 0.21 (vendored via `rsraw-sys`), camera white balance, AHD / Markesteijn demosaic,
  linear 16-bit, Rec.2020 primaries. No highlight recovery, no denoise, no lens correction.
- Film: [spektrafilm-rs](https://github.com/turbasvin/spektrafilm-rs) crates, wgpu/Metal backend.
- Every import writes a manifest (`<archive>/.spektrorganize/imports/*.json`) with hashes, capture
  metadata and the full preset, so renders can be regenerated after the card is gone or the archive
  has moved.

## Layout

```
crates/spektro-core     library: scan, templates, plan, copy, manifest, decode, film, export, job, catalog
crates/spektro-cli      `spektro` command
apps/desktop            Tauri 2 + Svelte 5 app
presets/                film presets (JSON: film, print, spektrafilm param overrides)
vendor/spektrafilm-data profiles + spectral LUT copied from spektrafilm-rs (scripts/sync-spektrafilm-data.sh)
```

## Build

Rust ≥ 1.88, Node ≥ 20, Xcode command line tools (LibRaw is compiled from source).

```bash
cargo build --release                 # CLI at target/release/spektro
cd apps/desktop && npm install && npm run tauri dev
```

## CLI

```bash
spektro init ~/.config/spektrorganize/config.toml   # then edit roots / templates / preset
spektro sources                                     # mounted cards
spektro scan /Volumes/CARD
spektro plan /Volumes/CARD                          # destination tree, nothing written
spektro import /Volumes/CARD [--no-render] [--exclude 3,7]
spektro render <manifest.json> --jpeg --exr [--render-root DIR] [--only-missing]
spektro decode IMG.RAF --exr out.exr --jpeg out.jpg  # honest decode, no film stage
spektro profiles
```

## Catalog

SQLite (WAL) index of everything the app knows about. Desktop app: `<app data>/catalog.sqlite`
(`~/Library/Application Support/dev.alexharding.spektrorganize/`), thumbnails in the app cache.
CLI: `--catalog PATH`, default the app's own `~/Library/Application Support/dev.alexharding.spektrorganize/catalog.sqlite`, so the CLI and the app share one library.

- **roots**: `archive` (written by imports), `folder` (added in place, never moved or renamed), or
  `render` (print outputs). Paths below a root are stored relative to it; `relocate` re-points a
  moved root. A disconnected drive is `offline`, not missing.
- **files** (size, mtime, BLAKE3 when known, kind) -> **asset_files** (role: `raw`, `jpeg` sidecar,
  `image`, `video`, `xmp`, `sidecar`) -> **assets** (the photo: capture time, camera, lens, ISO,
  size, rating). A RAW and its camera JPEG are one asset, grouped exactly like an import scan.
- Rescans are incremental (size + mtime). Vanished files stay as `missing` so keywords survive;
  a file that reappears elsewhere in the root with the same name, size and mtime keeps its asset.
- **keywords** per asset with a source: `user` or `ai:claude`. User keywords always win and are
  never touched by labelling; AI labels can be removed one by one or all at once.
- **thumbnails** (256 px in the background, 1024 px on demand), from the camera JPEG or the RAW's
  embedded preview; failures are remembered until the file changes.
- **renders** ("prints"): asset, preset name + hash, output path; **presets** keeps the preset JSON
  each print was made with. Imports, manifests and prints all record here.

Schema changes go through numbered migrations (`PRAGMA user_version`), see
`crates/spektro-core/src/catalog/schema.rs`.

```bash
spektro catalog add ~/Pictures/Old            # index a folder in place (again = incremental rescan)
spektro catalog adopt ~/Pictures/Archive      # an existing import archive, renders included (read-only)
spektro catalog ls --keyword beach --camera X-T10 --from 2024-10-01 --to 2024-10-31
spektro catalog show 42
spektro catalog tag 42 43 --add beach --remove test
spektro catalog print 42 43 --preset portra400_645.json --jpeg
spektro catalog roots | rescan <root> | relocate <root> <new path> | remove-root <root> | thumbs
```

`spektro import` records the import in the catalog too (`--no-catalog` to skip).

**AI labelling** (desktop only): ✦ Label sends each photo's 256 px thumbnail plus file name,
camera, date and folder to Claude (`claude-opus-5`) and stores the keywords as `ai:claude`. It
needs an Anthropic API key (`ANTHROPIC_API_KEY`, or saved in Settings to
`<app data>/anthropic_key`, mode 0600) and only runs when you press Label.

## Desktop app notes

The window sets `dragDropEnabled: false` in `tauri.conf.json`. Tauri's OS-level file-drop
handler otherwise swallows the webview's own drag events, which breaks the token chips in the
Layout templates. The app does not accept dropped files from Finder, so nothing is lost.

## Rating, select and reject

In the grid or the full-screen view: `1`–`5` give that many hearts, `0` clears, `S` marks a
select, `R` marks a reject (pressing the same key again clears it). With a selection, the keys
apply to every selected photo. The sidebar filters on Selected, Rejected and hearts. Zoom
presets moved to the numpad (`Numpad 0` fit, `Numpad 1` 1:1) so the digits are free to rate.

Ratings, flags and keywords live in the catalog database only — nothing is written to your photo
files, and no XMP sidecars are produced yet.

## Crop, rotate, straighten

Part of develop, stored with the photo (`RawSettings`) and applied to the decoded linear image
before the film stage, so a print is made from the frame you cropped. The photo file is never
touched. Order is fixed: quarter turns, then the straighten angle, then the crop.

Straightening trims to the largest rectangle of the original aspect that fits inside the rotated
frame, so there are never empty corners, and the crop is a fraction of what is left. In Develop:
⟲/⟳ for 90°, a straighten slider (double-click to level), and **crop** for the rectangle — drag
on the photo, with Free / Original / 1:1 / 3:2 / 2:3 / 4:3 / 3:4 / 16:9 to hold a shape. While
the crop tool is open the preview shows the whole frame; closing it shows the crop.

## Duplicates on import

A scan checks itself and the library before anything is copied. Photos that are byte-identical to
another on the same card — a folder copied into its own sub-folder scans as two of everything —
collapse to one: the copy nearest the top of the tree is imported, the rest are excluded and
badged "copy on this card". Photos whose bytes the catalog already holds are badged "already in
the library" and excluded too. One button puts them all back if you disagree.

Only files that share a *name and size* with something else are read, so a card of new photos is
checked for the price of one query.

## Duplicates

Sidebar → **Duplicates…** finds files that are identical byte for byte (same size, same BLAKE3)
and nothing else, because that is the only case where removing a copy loses nothing. A RAW and
its camera JPEG are different files and never appear. Hashes are computed only for files whose
size collides with another's, so a library of unique photos finishes without reading anything.

Pick the copy to keep per group; the rest go to the **Trash** (a "delete for good" switch skips
it). Each purge is confirmed with the full paths first. Before a copy goes, its keywords, rating,
flag and prints are merged onto the photo you keep, and a photo left with no files at all is
dropped from the catalog. The backend refuses to remove a file that is not a verified copy of the
one being kept, or to act at all when the keeper is not on disk — so it cannot take your last
copy. This is the only part of the app that touches a photo on disk.

## Purging rejects

Sidebar → **Purge rejected…** shows every photo flagged ✕ as thumbnails, all chosen by default;
click one to spare it. Both this and the duplicate purge end at the same screen: every path that
will go, in full, next to the ones that stay, with a single button to do it. Trash by default, a
"delete for good" switch beside it. Every file of a photo goes together — the RAW, its camera
JPEG, its sidecars — and prints already made from it stay on disk.

## Dynamic catalogs

A dynamic catalog is a saved filter, stored in the `collections` table (schema v6) and re-run
every time it is opened — nothing is copied, and photos join or leave it as they are rated,
tagged or printed. Filter the library, press + in the sidebar's Catalogs section and name it;
saving the same name replaces it. The sort is saved with the filter when one is set.

Dates filter on any range: the Dates section has From / To pickers above the year/month
drilldown, and the range is part of what a catalog remembers.

## Prints in the library

Every print is linked to its photo (`renders.asset_id`), so the grid badges it, the sidebar can
filter on it, and the full-screen viewer steps through the camera rendition and each print with
`[` and `]`. Settings → Catalog has "Show prints in the library": with it on, a photo's thumbnail
is made from its newest JPEG print instead of the camera rendition, and re-printing refreshes it
(the thumbnail key follows the print file's mtime).

Renders live wherever you keep them, which is outside the webview's file scope, so the app serves
a cached copy from its own cache directory (`catalog_render_preview`) rather than widening that
scope.

## Templates

`{token}` / `{token:arg}`; `{{` for a literal brace. Tokens: `date[:strftime]`, `year`, `month`,
`day`, `time`, `import_date[:strftime]`, `camera`, `make`, `model`, `stem`, `ext[:lower]`, `kind`,
`volume`, `rel_dir`, `seq[:width]`, `iso`, `lens`, `preset` (render templates). Separate templates for RAW, paired JPEG, lone
JPEG, video, other, rendered EXR, rendered JPEG.

## License

GPL-3.0 (spektrafilm-rs is GPL-3.0). LibRaw is LGPL-2.1/CDDL. Film profiles are CC BY-SA 4.0
from spektrafilm.

## Later

Develop-stage settings the decoder supports but the UI does not expose yet (deferred
2026-09-19): grey-picker white balance (`user_mul`) and auto white balance; black point
and white level (`user_black` / `user_sat`); the seven highlight-rebuild levels; exposure
applied during decode with highlight preservation (`exp_correc` / `exp_shift` /
`exp_preser`) instead of the current post-decode multiply; noise reduction (`threshold`,
`fbdd_noiserd`); chromatic-aberration scaling (`aber`); median passes; white-balance
sample area (`greybox`); camera-matrix choice (file vs LibRaw table — matters for bodies
LibRaw does not know, e.g. the A7C II); four-colour and DCB demosaic options; frame
selection in multi-frame RAWs; bad-pixel map and dark frame. Not decoder settings but
arguably the same stage: crop and straighten, developed-file colour space / bit depth,
lens corrections.

The layout preview colours folders by where they come from: a folder written literally in a
template stays white, one a token produced takes the colour of its depth (`--tok-1`…`--tok-6`
in `app.css`, muted filmic tones). Those six are not in the theme picker yet.
