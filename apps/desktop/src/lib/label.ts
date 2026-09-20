// AI keyword labelling for catalog photos, the way organisicator does it: Claude looks at each
// photo's 256 px thumbnail plus its file name, camera, date and folder, and returns search
// keywords. They are stored as keywords with source "ai:claude" (✦ chips) so they can be told
// apart and cleared; the user's own keywords are never touched.
//
// Runs only when the user presses ✦ Label, and only with an API key (ANTHROPIC_API_KEY or the
// key saved in Settings). The key stays on this machine; the webview calls the API directly.

import Anthropic from "@anthropic-ai/sdk";
import { z } from "zod";
import { betaZodOutputFormat } from "@anthropic-ai/sdk/helpers/beta/zod";
import type { LabelInfo } from "./api";

const MODEL = "claude-opus-5";
export const LABEL_SOURCE = "ai:claude";
export const LABEL_BATCH = 20;
const MAX_KEYWORDS = 8;

export const LabelsSchema = z.object({
  labels: z.array(
    z.object({
      i: z.number().int(),
      keywords: z.array(z.string()),
    }),
  ),
});
export type LabelsOutput = z.infer<typeof LabelsSchema>;

/** Words that say nothing a search would want (or that the catalog already knows). */
const GENERIC = new Set([
  "photo", "photos", "photograph", "image", "images", "picture", "pic", "file", "raw", "jpeg", "jpg",
  "camera", "digital photo", "untitled", "misc", "thumbnail",
]);

/** One keyword: lowercase, single-spaced, 1–3 words, no stray punctuation. Null when empty,
 * generic or too long to be a keyword. */
export function cleanKeyword(k: string): string | null {
  const s = k
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s_\-.']/gu, " ")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/^[-_.']+|[-_.']+$/g, "");
  if (!s || s.length > 32) return null;
  if (s.split(" ").length > 3) return null;
  if (GENERIC.has(s)) return null;
  return s;
}

/** Cleans, de-duplicates and caps a keyword list; `skip` holds words that would only repeat what
 * is known (the user's own keywords, the camera). */
export function cleanKeywords(raw: string[], skip: Set<string> = new Set()): string[] {
  const out: string[] = [];
  for (const k of raw) {
    const c = cleanKeyword(k);
    if (!c || skip.has(c) || out.includes(c)) continue;
    out.push(c);
    if (out.length >= MAX_KEYWORDS) break;
  }
  return out;
}

/** Maps the model's output back onto the batch; out-of-range indices and empty results drop. */
export function labelsFromOutput(out: LabelsOutput, items: LabelInfo[]): { id: number; keywords: string[] }[] {
  const byId = new Map<number, string[]>();
  for (const l of out.labels) {
    const it = items[l.i];
    if (!it) continue;
    const skip = new Set([...it.keywords.map((k) => k.toLowerCase()), (it.camera ?? "").toLowerCase()]);
    const tags = cleanKeywords([...(byId.get(it.id) ?? []), ...l.keywords], skip);
    if (tags.length) byId.set(it.id, tags);
  }
  return [...byId].map(([id, keywords]) => ({ id, keywords }));
}

function itemLine(it: LabelInfo, i: number): string {
  const bits = [`[${i}] ${it.name}`, it.kind === "raw" ? "raw photo" : it.kind];
  if (it.captured_at) bits.push(it.captured_at.replace("T", " "));
  if (it.camera) bits.push(it.camera);
  if (it.lens) bits.push(it.lens);
  const where = [it.root, it.folder].filter(Boolean).join("/");
  if (where) bits.push(`folder: ${where}`);
  if (it.keywords.length) bits.push(`owner's keywords: ${it.keywords.join(", ")}`);
  return bits.join(" · ");
}

export const LABEL_INSTRUCTIONS = `You label photos in a personal photo library with search keywords.

Each photo comes with its file name, capture date and time, camera, lens and folder, followed by a small thumbnail when one exists.

For each photo give 3–${MAX_KEYWORDS} lowercase keywords (1–3 words each) its owner might search for later:
- the main subject (person, dog, building, bridge, food, boat…) and notable objects
- the setting and scene (street, beach, forest, kitchen, city skyline, mountain lake…)
- activity or event when visible (wedding, hiking, concert, market…)
- light, time of day, weather and season when clear (golden hour, night, fog, snow, backlit…)
- a place name only when it is unmistakable from the picture or the folder name
Do not repeat the camera, lens, date or file name (those are searchable already) or the owner's keywords, and avoid generic words (photo, image). Use the thumbnail; without one, infer only what the name and folder say and do not guess content you cannot see. Return one entry per index.`;

type Block =
  | { type: "text"; text: string }
  | { type: "image"; source: { type: "base64"; media_type: "image/jpeg" | "image/png" | "image/webp" | "image/gif"; data: string } };

/** A data: URL -> image block (null for anything that isn't a base64 image). */
export function imageBlock(dataUrl: string | null | undefined): Block | null {
  const m = /^data:(image\/(?:png|jpeg|webp|gif));base64,(.+)$/.exec(dataUrl ?? "");
  if (!m) return null;
  return { type: "image", source: { type: "base64", media_type: m[1] as "image/jpeg", data: m[2] } };
}

/** Labels `items` in batches, handing each batch to `onBatch` as soon as it lands (so a stop or
 * failure keeps what was done). */
export async function labelItems(
  apiKey: string,
  items: LabelInfo[],
  thumb: (it: LabelInfo) => Promise<string | null>,
  onBatch: (labels: { id: number; keywords: string[] }[], done: number, total: number) => Promise<void>,
  isCancelled: () => boolean = () => false,
): Promise<{ refused: number }> {
  // Local personal app: the key stays on this machine.
  const client = new Anthropic({ apiKey, dangerouslyAllowBrowser: true });
  const total = Math.ceil(items.length / LABEL_BATCH);
  let refused = 0;
  for (let b = 0; b < total; b++) {
    if (isCancelled()) break;
    const slice = items.slice(b * LABEL_BATCH, (b + 1) * LABEL_BATCH);
    const thumbs = await Promise.all(slice.map((it) => thumb(it).catch(() => null)));
    const content: Block[] = [];
    slice.forEach((it, i) => {
      content.push({ type: "text", text: itemLine(it, i) });
      const img = imageBlock(thumbs[i]);
      if (img) content.push(img);
    });
    content.push({ type: "text", text: `Label photos 0–${slice.length - 1}.` });
    const response = await client.beta.messages.parse({
      model: MODEL,
      max_tokens: 8000,
      system: LABEL_INSTRUCTIONS,
      // A declined request is re-run server-side on Anthropic's recommended fallback model.
      betas: ["server-side-fallback-2026-07-01"],
      fallbacks: "default",
      output_config: { effort: "low", format: betaZodOutputFormat(LabelsSchema) },
      messages: [{ role: "user", content }],
    });
    if (isCancelled()) break;
    if (response.stop_reason === "refusal" || !response.parsed_output) {
      // Skip this batch, keep going; the photos stay unlabelled and can be retried.
      refused += slice.length;
      await onBatch([], b + 1, total);
      continue;
    }
    await onBatch(labelsFromOutput(response.parsed_output, slice), b + 1, total);
  }
  return { refused };
}
