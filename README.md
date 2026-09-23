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

## Sony bodies LibRaw does not know

LibRaw 0.21 has no entry for the A7C II and its generation, so it decodes the whole raw frame
with the sensor's masked border still on — 7168×5120 for a 7008×4672 picture, the extra as a
black band down the right and along the bottom. The file records where the picture is
(`raw_inset_crops`); the decode crops to that when it is smaller than LibRaw's frame, and the
catalog records that size. Colour still borrows the nearest known body's matrix; only a newer
LibRaw fixes that.

Drives are re-checked every few seconds, so a root that was offline comes back as soon as the
volume mounts.

## Rating, select and reject

In the grid or the full-screen view: `1`–`5` give that many hearts, `0` clears, `S` marks a
select, `R` marks a reject (pressing the same key again clears it). With a selection, the keys
apply to every selected photo. The sidebar filters on Selected, Rejected and hearts. Zoom
presets moved to the numpad (`Numpad 0` fit, `Numpad 1` 1:1) so the digits are free to rate.

Ratings, flags and keywords live in the catalog database only — nothing is written to your photo
files, and no XMP sidecars are produced yet.

## The Print panel

Grouped as spektrafilm's own Flow panel is — Film, Print, DIR couplers, Grain, Halation,
Diffusion, Scanner — so a look reads the same in both apps, with this app's extra controls at the
tail of each group. Every path the panel binds is checked against the parameter model by a test.
What Flow shows that this rev of spektrafilm-rs has no field for (push/pull, shadow and highlight
shape, print timing, output role, colour adaptation, grain model and saturation, lens correction)
waits on a dependency bump.

## The viewport

Develop and Print draw the picture on a WebGPU canvas (WebGL2 when that is missing), not with an
`<img>`. A rendered frame comes from the app as raw RGBA bytes over `frame://` and goes straight
into a texture: no JPEG, no base64, no compositor. Zoom and pan are two numbers the shader reads.
Past one texel per device pixel the sampler is nearest, so pixels are square; below it, linear
over mipmaps, so a full-resolution frame fitted to the window does not shimmer. Numpad 1 is one
picture pixel per screen pixel; the badge reads in the same terms.

The rest of the app stays HTML, which is what it is good at. The picture is the one thing the
DOM was never the right surface for.

## Crop, rotate, straighten

Part of develop, stored with the photo (`RawSettings`) and applied to the decoded linear image
before the film stage, so a print is made from the frame you cropped. The photo file is never
touched. Order is fixed: quarter turns, then the straighten angle, then the crop.

Straightening trims to the largest rectangle of the original aspect that fits inside the rotated
frame, so there are never empty corners, and the crop is a fraction of what is left. In Develop:
⟲/⟳ for 90°, a straighten slider (double-click to level), and **crop** for the rectangle — drag
on the photo, with Free / Original / 1:1 / 3:2 / 2:3 / 4:3 / 3:4 / 16:9 to hold a shape. While
the crop tool is open the preview shows the whole frame; closing it shows the crop.

## Video

macOS only, through AVFoundation: the system decodes and encodes, so there is no bundled codec,
no HEVC licensing question, and hardware acceleration where the machine has it. On other
platforms the calls return `Unsupported` and the app builds without video.

Clips get poster frames, duration, size, codec and the container's own creation date. HEVC poster
frames can be turned off (Settings → Catalog) for libraries where decoding H.265 is not worth it.
Playback streams over a `clip://` protocol the app serves itself, by catalog file id and with
byte ranges, because the webview cannot read the archive and widening its file access would be
worse.

A clip **previews** in Develop and Print like a photo, on one of its frames: the same look, the
same before/after toggle, the same zoom. A frame slider under the stage picks which one — the
poster frame by default, which is a tenth of the way in. Scrubbing waits for the handle to settle
before decoding.

Clips **print** like photos: select them and press Print, and each is rendered into the render
root under the render template's name (with the codec's extension), recorded as a `renders` row
and badged like any other print. The print dialog gains the clip options when the selection holds
any. A single clip can also be rendered to a path you choose — right-click → Render this clip, or
Render… in the full-screen view.

**Rendering a look onto a clip** (Render… in the full-screen view) comes two ways:

- **Baked LUT** — the preset is run once over a colour cube and applied per frame. Fast enough
  for a whole clip, and the cube exports as `.cube` for Resolve. It cannot carry grain or
  halation: those are spatial, and a cube maps colour to colour.
- **Full pipeline** — spektrafilm on every frame, grain and all. Slower, and the honest one.

Either way the exposure is metered once on a reference frame and held for the clip; metering each
frame separately makes the picture pump as the scene changes. Output is H.264 or HEVC in MP4, or
ProRes 422/4444 in a QuickTime movie, at the clip's size or capped for posting.

Known gap: **rendered clips are silent.** An AVAssetWriter interleaves its inputs and every
arrangement tried so far ends with one input waiting on the other, so rather than ship a hang the
renderer reports `silent`.

## Image sequences

A folder of `render_0001.png … render_0480.png` is one shot, not 480 photos. The import review
collapses numbered runs (via the [`sequitur`](https://crates.io/crates/sequitur) crate) into one
row each, showing the pattern, the frame count and any gaps; four numbered files is the floor, so
`DSCF0001..0003` stays three photos. Collapsing is only how the review reads — every frame is
still imported, and one click includes or excludes the whole run.

## Import descriptions

The Layout screen has a note for what an import is — a shoot, a trip, a roll. It goes into the
manifest and onto every photo the import brings in, so it survives the card, the folder names and
any later re-organisation.

## Duplicates on import

A scan checks itself and the library before anything is copied. Photos that are byte-identical to
another on the same card — a folder copied into its own sub-folder scans as two of everything —
collapse to one: the copy nearest the top of the tree is imported, the rest are excluded and
badged "copy on this card". Photos whose bytes the catalog already holds are badged "already in
the library" and excluded too. One button puts them all back if you disagree.

The catalog side costs nothing to check: an import preserves a file's modification time, so a
photo already in the library has the same name, size *and* mtime as the one on the card, and that
settles it without reading either. Only when those agree but the moment differs, and the catalog
recorded a hash, are bytes compared. Within the scan itself, files sharing a name and size are
hashed — that is the folder-copied-into-itself case, and it is bounded by how many copies there
are.

What this gives up: a copy that was indexed in place (so has no stored hash) and has a different
modification time is not spotted at scan time. The Duplicates scan in the library catches that
case, because there it can afford to read.

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
