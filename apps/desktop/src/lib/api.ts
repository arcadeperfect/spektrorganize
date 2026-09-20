import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Kind = "raw" | "image" | "video" | "sidecar" | "other";
export type PreviewSource = "sidecar_jpeg" | "embedded_preview" | "itself" | "none";

export interface SourceInfo {
  root: string;
  label: string;
  is_volume: boolean;
}

export interface GroupView {
  id: number;
  kind: Kind;
  name: string;
  rel: string;
  bytes: number;
  attachments: string[];
  captured_at: string | null;
  camera: string | null;
  iso: number | null;
  lens: string | null;
  preview: PreviewSource;
  excluded: boolean;
}

export interface ScanView {
  source: SourceInfo;
  groups: GroupView[];
  files: number;
  bytes: number;
  errors: [string, string][];
}

export type DestStatus =
  | { status: "new" }
  | { status: "exists_same_size" }
  | { status: "exists_different" }
  | { status: "collision" };

export interface TreeNode {
  name: string;
  is_dir: boolean;
  files: number;
  bytes: number;
  status: DestStatus | null;
  /** A folder a template token produced, rather than one written literally. */
  dynamic: boolean;
  children: TreeNode[];
}

export interface PlanView {
  tree: TreeNode[];
  files: number;
  bytes: number;
  bytes_to_copy: number;
  renders: number;
  collisions: number;
  existing: number;
}

export interface Templates {
  raw: string;
  paired_image: string;
  image: string;
  video: string;
  other: string;
  render_exr: string;
  render_jpeg: string;
}

export interface Outputs {
  exr: boolean;
  jpeg: boolean;
  exr_color_space: string;
  jpeg_quality: number;
}

export interface Config {
  archive_root: string;
  video_root: string | null;
  other_root: string | null;
  render_root: string;
  templates: Templates;
  outputs: Outputs;
  preset: string;
  data_dir: string | null;
  verify: "hash" | "size_only";
  skip_existing: boolean;
  decode_threads: number;
}

export interface PresetSummary {
  name: string;
  film: string;
  print: string;
  path: string;
}

export interface ManifestSummary {
  path: string;
  created_at: string;
  source_label: string;
  entries: number;
  raws: number;
  rendered: number;
  preset: string | null;
}

export interface Info {
  config_path: string;
  cache_dir: string;
  data_dir: string | null;
  libraw_version: string;
  spektrafilm_rev: string;
}

export type CopyStatus =
  | { status: "copied" }
  | { status: "skipped_existing" }
  | { status: "failed"; detail: string };

export type JobEvent =
  | { type: "copy_started"; files: number; bytes: number }
  | { type: "file_started"; file: number; dest: string; size: number }
  | { type: "file_progress"; file: number; bytes_done: number }
  | { type: "file_done"; file: number; status: CopyStatus }
  | { type: "copy_finished"; copied: number; skipped: number; failed: number; bytes: number }
  | { type: "manifest_written"; path: string }
  | { type: "render_started"; total: number; backend: string }
  | { type: "render_done"; entry: number; outputs: string[]; seconds: number }
  | { type: "render_failed"; entry: number; error: string }
  | { type: "render_finished"; done: number; failed: number }
  | { type: "render_skipped"; reason: string }
  | { type: "log"; 0: string };

// ---------- catalog ----------

export type RootKind = "archive" | "folder" | "render";
export type SortKey = "captured_desc" | "captured_asc" | "added_desc" | "name";

export interface CatalogRoot {
  id: number;
  path: string;
  kind: RootKind;
  label: string;
  added_at: string;
  online: boolean;
  last_indexed_at: string | null;
  assets: number;
  files: number;
  missing: number;
  renders: number;
}

export interface Filter {
  roots?: number[];
  date_from?: string | null;
  date_to?: string | null;
  cameras?: string[];
  keywords?: string[];
  has_render?: boolean | null;
  kinds?: string[];
  text?: string | null;
  min_rating?: number | null;
  ai_labelled?: boolean | null;
  missing?: boolean | null;
  undated?: boolean;
  flags?: string[];
}

export interface AssetSummary {
  id: number;
  kind: string;
  name: string;
  captured_at: string | null;
  camera: string | null;
  rating: number;
  flag: string;
  width: number | null;
  height: number | null;
  files: number;
  has_jpeg: boolean;
  renders: number;
  ai_labelled: boolean;
  thumb: string | null;
  missing: boolean;
  online: boolean;
  root_id: number | null;
}

export interface Page {
  total: number;
  offset: number;
  items: AssetSummary[];
}

export interface Count {
  key: string;
  count: number;
}

export interface Facets {
  total: number;
  undated: number;
  rendered: number;
  ai_labelled: number;
  missing: number;
  selected: number;
  rejected: number;
  rated: number;
  cameras: Count[];
  kinds: Count[];
  months: Count[];
  keywords: { name: string; count: number; ai: number }[];
  roots: CatalogRoot[];
}

/** A dynamic catalog: a saved filter, re-run every time it is opened. */
export interface Collection {
  id: number;
  name: string;
  filter: Filter;
  sort: SortKey | null;
  created_at: string;
}

export interface AssetKeyword {
  name: string;
  source: string;
  confidence: number | null;
}

export interface FileInfo {
  id: number;
  role: string;
  kind: string;
  name: string;
  rel: string;
  path: string;
  root_id: number;
  online: boolean;
  size: number;
  blake3: string | null;
  missing: boolean;
}

export interface RenderInfo {
  id: number;
  kind: string;
  preset_name: string | null;
  preset_hash: string;
  path: string;
  created_at: string;
  exists: boolean;
}

export interface AssetDetail {
  id: number;
  kind: string;
  captured_at: string | null;
  make: string | null;
  model: string | null;
  camera: string | null;
  lens: string | null;
  iso: number | null;
  width: number | null;
  height: number | null;
  orientation: number | null;
  meta_source: string | null;
  rating: number;
  flag: string;
  added_at: string;
  primary_file_id: number | null;
  import_source: string | null;
  imported_at: string | null;
  files: FileInfo[];
  keywords: AssetKeyword[];
  renders: RenderInfo[];
  thumb: string | null;
  preview: string | null;
}

export interface LabelInfo {
  id: number;
  kind: string;
  name: string;
  folder: string;
  root: string;
  captured_at: string | null;
  camera: string | null;
  lens: string | null;
  keywords: string[];
  ai_keywords: string[];
}

export interface ThumbReady {
  id: number;
  size: number;
  path: string | null;
}

export type IndexProgress =
  | { phase: "walking"; files: number }
  | { phase: "metadata"; done: number; total: number }
  | { phase: "writing"; done: number; total: number }
  | { phase: "manifest"; done: number; total: number; name: string };

export interface CatalogInfo {
  path: string;
  thumbs_dir: string;
  assets: number;
  schema: number;
}

export const catalog = {
  info: () => invoke<CatalogInfo>("catalog_info"),
  facets: () => invoke<Facets>("catalog_facets"),
  list: (filter: Filter, sort: SortKey, offset: number, limit: number) =>
    invoke<Page>("catalog_list", { filter, sort, offset, limit }),
  ids: (filter: Filter, sort: SortKey) => invoke<number[]>("catalog_ids", { filter, sort }),
  asset: (id: number) => invoke<AssetDetail>("catalog_asset", { id }),
  addFolder: (path: string) => invoke<void>("catalog_add_folder", { path }),
  adopt: (path: string | null) => invoke<void>("catalog_adopt", { path }),
  rescan: (root: number) => invoke<void>("catalog_rescan", { root }),
  cancelIndex: () => invoke<void>("catalog_cancel_index"),
  relocateRoot: (root: number, path: string, force: boolean) =>
    invoke<{ checked: number; found: number }>("catalog_relocate_root", { root, path, force }),
  removeRoot: (root: number) => invoke<void>("catalog_remove_root", { root }),
  requestThumbs: (ids: number[], force = false) => invoke<ThumbReady[]>("catalog_request_thumbs", { ids, force }),
  preview: (id: number) => invoke<string | null>("catalog_preview", { id }),
  previewPx: (id: number, maxPx: number) => invoke<string | null>("catalog_preview_px", { id, maxPx }),
  renderPreview: (render: number, maxPx: number) => invoke<string | null>("catalog_render_preview", { render, maxPx }),
  collections: () => invoke<Collection[]>("catalog_collections"),
  saveCollection: (name: string, filter: Filter, sort: SortKey | null) =>
    invoke<number>("catalog_save_collection", { name, filter, sort }),
  deleteCollection: (id: number) => invoke<void>("catalog_delete_collection", { id }),
  thumbsFromPrints: () => invoke<boolean>("catalog_thumbs_from_prints"),
  setThumbsFromPrints: (on: boolean) => invoke<void>("catalog_set_thumbs_from_prints", { on }),
  thumbData: (id: number) => invoke<string | null>("catalog_thumb_data", { id }),
  tag: (ids: number[], add: string[], remove: string[]) => invoke<void>("catalog_tag", { ids, add, remove }),
  setRating: (ids: number[], rating: number) => invoke<void>("catalog_set_rating", { ids, rating }),
  setFlag: (ids: number[], flag: string | null) => invoke<void>("catalog_set_flag", { ids, flag }),
  labelInfo: (ids: number[]) => invoke<LabelInfo[]>("catalog_label_info", { ids }),
  applyLabels: (labels: { id: number; keywords: string[] }[], source: string, replace: boolean) =>
    invoke<number>("catalog_apply_labels", { labels, source, replace }),
  clearLabels: (ids: number[], prefix = "ai:") => invoke<number>("catalog_clear_labels", { ids, prefix }),
  print: (req: { ids: number[]; preset: string; jpeg: boolean; exr: boolean; camera_jpeg?: boolean }) =>
    invoke<void>("catalog_print", { req }),
  queue: () => invoke<number[]>("catalog_queue"),
  queueAdd: (ids: number[]) => invoke<number>("catalog_queue_add", { ids }),
  queueRemove: (ids: number[]) => invoke<void>("catalog_queue_remove", { ids }),
  queueClear: () => invoke<void>("catalog_queue_clear"),
  export: (req: {
    ids: number[];
    look: string | null;
    jpeg: boolean;
    exr: boolean;
    camera_jpeg: boolean;
    destination: string | null;
    clear_queue: boolean;
  }) => invoke<void>("catalog_export", { req }),
  aiKey: () => invoke<string | null>("ai_key"),
  aiKeySave: (key: string) => invoke<boolean>("ai_key_save", { key }),
};

export const api = {
  info: () => invoke<Info>("info"),
  listSources: () => invoke<SourceInfo[]>("list_sources"),
  getConfig: () => invoke<Config>("get_config"),
  setConfig: (config: Config) => invoke<void>("set_config", { config }),
  defaultTemplates: () => invoke<Templates>("default_templates"),
  validateTemplate: (template: string) => invoke<string[]>("validate_template", { template }),
  listPresets: () => invoke<PresetSummary[]>("list_presets"),
  startScan: (path: string) => invoke<void>("start_scan", { path }),
  getScan: () => invoke<ScanView | null>("get_scan"),
  setExcluded: (ids: number[], excluded: boolean) => invoke<void>("set_excluded", { ids, excluded }),
  thumbnail: (group: number) => invoke<string | null>("thumbnail", { group }),
  makePlan: () => invoke<PlanView>("make_plan"),
  startImport: (render: boolean) => invoke<void>("start_import", { render }),
  startRenderManifest: (req: {
    manifest: string;
    exr: boolean;
    jpeg: boolean;
    render_root: string | null;
    only_missing: boolean;
  }) => invoke<void>("start_render_manifest", { req }),
  cancelJob: () => invoke<void>("cancel_job"),
  isBusy: () => invoke<boolean>("is_busy"),
  listManifests: () => invoke<ManifestSummary[]>("list_manifests"),
  reveal: (path: string) => invoke<void>("reveal", { path }),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  fileUrl: (path: string) => convertFileSrc(path),
  on: <T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> =>
    listen<T>(event, (e) => handler(e.payload)),
};

export function human(bytes: number): string {
  const u = ["B", "KB", "MB", "GB", "TB"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${bytes} B` : `${v.toFixed(1)} ${u[i]}`;
}

// ---------- looks (film presets) ----------

export interface StockInfo {
  name: string;
  label: string;
  positive: boolean;
  bw: boolean;
  usage: string;
  target_print: string | null;
}

export interface LookMeta {
  films: StockInfo[];
  papers: StockInfo[];
  color_filters: { value: string; label: string }[];
  color_spaces: string[];
  upsamplers: string[];
  routes: string[];
  gamut_algorithms: string[];
}

/** A look: film + paper + only the parameters it changes (layered on the film's stock defaults). */
export interface Look {
  name: string;
  film: string;
  print: string;
  params: Record<string, any>;
}

export type WhiteBalance = "as_shot" | "daylight" | "tungsten" | "custom";

export interface RawSettings {
  white_balance: WhiteBalance;
  temperature: number;
  tint: number;
  exposure_ev: number;
  highlight: number;
  demosaic: number | null;
}

export const defaultRaw = (): RawSettings => ({
  white_balance: "as_shot",
  temperature: 5500,
  tint: 1,
  exposure_ev: 0,
  highlight: 0,
  demosaic: null,
});

export interface ImportedLook {
  preset: Look;
  warnings: string[];
}

export interface InputInfo {
  is_raw: boolean;
  file: string;
  error: string | null;
  width: number | null;
  height: number | null;
}

/** Preset reference meaning "develop only, no film look". */
export const DEVELOPED_ONLY = "__developed__";

export const looks = {
  input: (id: number) => invoke<InputInfo>("asset_input", { id }),
  meta: () => invoke<LookMeta>("looks_meta"),
  load: (path: string) => invoke<Look>("look_load", { path }),
  save: (preset: Look, path: string | null) => invoke<string>("look_save", { preset, path }),
  remove: (path: string) => invoke<void>("look_delete", { path }),
  importFile: (path: string) => invoke<ImportedLook>("look_import_file", { path }),
  importUrl: (url: string) => invoke<ImportedLook>("look_import_url", { url }),
  importText: (text: string) => invoke<ImportedLook>("look_import_text", { text }),
  resolve: (preset: Look) => invoke<Record<string, any>>("look_resolve", { preset }),
  neutralize: (preset: Look) => invoke<[number, number]>("look_neutralize", { preset }),
  preview: (id: number, preset: Look, raw: RawSettings, maxPx: number) =>
    invoke<string>("look_preview", { id, preset, raw, maxPx }),
  developPreview: (id: number, raw: RawSettings, maxPx: number) => invoke<string>("develop_preview", { id, raw, maxPx }),
  previewBefore: (id: number, preset: Look, raw: RawSettings, maxPx: number) =>
    invoke<string>("look_preview_before", { id, preset, raw, maxPx }),
  rawGet: (id: number) => invoke<RawSettings>("asset_raw_get", { id }),
  rawSet: (ids: number[], raw: RawSettings) => invoke<void>("asset_raw_set", { ids, raw }),
};
