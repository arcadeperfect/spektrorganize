// Path templates as parts, so the UI can show tokens as chips.
// `{token}` / `{token:arg}` become token parts; everything else is text.
// `{{` and `}}` are literal braces and stay inside text parts.

export type Part = { kind: "text"; value: string } | { kind: "token"; name: string; arg?: string };

/** Tokens the renderer understands, with the arguments that make sense. */
export const TOKENS: { name: string; args?: string[]; hint: string }[] = [
  { name: "date", args: ["%Y-%m-%d", "%Y", "%Y-%m", "%y%m%d"], hint: "capture date" },
  { name: "year", hint: "capture year" },
  { name: "month", hint: "capture month" },
  { name: "day", hint: "capture day" },
  { name: "time", args: ["%H%M%S", "%H-%M"], hint: "capture time" },
  { name: "import_date", args: ["%Y-%m-%d", "%Y"], hint: "date of the import" },
  { name: "camera", hint: "camera model, tidied" },
  { name: "make", hint: "camera maker" },
  { name: "model", hint: "camera model, raw" },
  { name: "stem", hint: "file name without extension" },
  { name: "ext", args: ["lower", "upper"], hint: "file extension" },
  { name: "kind", hint: "raw / image / video / other" },
  { name: "volume", hint: "card or folder name" },
  { name: "rel_dir", hint: "folder path on the card" },
  { name: "seq", args: ["4", "3", "5"], hint: "counter within the import" },
  { name: "iso", hint: "ISO" },
  { name: "lens", hint: "lens" },
  { name: "preset", hint: "look used for the render" },
];

export function parse(template: string): Part[] {
  const parts: Part[] = [];
  let text = "";
  for (let i = 0; i < template.length; i++) {
    const c = template[i];
    if (c === "{" && template[i + 1] === "{") {
      text += "{{";
      i++;
    } else if (c === "}" && template[i + 1] === "}") {
      text += "}}";
      i++;
    } else if (c === "{") {
      const end = template.indexOf("}", i);
      if (end === -1) {
        text += template.slice(i);
        break;
      }
      const inner = template.slice(i + 1, end);
      const colon = inner.indexOf(":");
      parts.push({ kind: "text", value: text });
      text = "";
      parts.push(
        colon === -1
          ? { kind: "token", name: inner.trim() }
          : { kind: "token", name: inner.slice(0, colon).trim(), arg: inner.slice(colon + 1) },
      );
      i = end;
    } else {
      text += c;
    }
  }
  parts.push({ kind: "text", value: text });
  return normalise(parts);
}

export function serialize(parts: Part[]): string {
  return parts
    .map((p) => (p.kind === "text" ? p.value : p.arg ? `{${p.name}:${p.arg}}` : `{${p.name}}`))
    .join("");
}

/** Text slots on both ends and between every pair of tokens, so there is always
 * somewhere to type a separator. Adjacent text parts merge, so an insert never
 * leaves an empty slot behind. */
export function normalise(parts: Part[]): Part[] {
  const out: Part[] = [];
  for (const p of parts) {
    const prev = out[out.length - 1];
    if (p.kind === "text") {
      if (prev && prev.kind === "text") out[out.length - 1] = { kind: "text", value: prev.value + p.value };
      else out.push({ ...p });
    } else {
      if (!prev || prev.kind !== "text") out.push({ kind: "text", value: "" });
      out.push(p);
    }
  }
  const last = out[out.length - 1];
  if (!last || last.kind !== "text") out.push({ kind: "text", value: "" });
  return out;
}

/**
 * Put a token into `parts` at `at`, with a "/" on either side where it would
 * otherwise butt straight up against a neighbour — dropping a token means
 * "another folder level", so the separator is what you want nearly every time.
 */
export function insertToken(parts: Part[], at: number, tok: Part): Part[] {
  // Nothing before the insertion point means this is the start of the path: a
  // separator there would make the template absolute, which is not allowed.
  const filled = parts.slice(0, at).some((p) => (p.kind === "token" ? true : p.value !== ""));
  const before = parts[at - 1];
  const joined = before && before.kind === "text" && SEP.test(before.value);
  const pre: Part[] = filled && !joined ? [{ kind: "text", value: "/" }] : [];
  // What follows: a chip, or an empty slot with a chip behind it, needs a separator too.
  const after = parts[at];
  const emptySlot = after && after.kind === "text" && after.value === "";
  const nextIsToken = after?.kind === "token" || (emptySlot && parts[at + 1]?.kind === "token");
  const post: Part[] = nextIsToken ? [{ kind: "text", value: "/" }] : [];
  const out = [...parts];
  out.splice(at, 0, ...pre, tok, ...post);
  return normalise(out);
}

/** Trailing separator, e.g. the "/" a chip sits behind. */
const SEP = /[/.\-_ ]$/;

/**
 * Take the token at `i` out, closing up behind it: the separators on either side
 * collapse to one, and a path left with nothing but punctuation clears itself.
 */
export function removeToken(parts: Part[], i: number): Part[] {
  const out = [...parts];
  const left = out[i - 1];
  const right = out[i + 1];
  out.splice(i, 1);
  if (left?.kind === "text" && right?.kind === "text" && SEP.test(left.value) && /^[/.\-_ ]/.test(right.value)) {
    out[i] = { kind: "text", value: right.value.slice(1) };
  }
  if (!out.some((p) => p.kind === "token")) {
    const rest = out.map((p) => (p.kind === "text" ? p.value : "")).join("");
    if (/^[/.\-_ ]*$/.test(rest)) return [{ kind: "text", value: "" }];
  }
  return normalise(out);
}

/** Split a template into its folders and its file name, ignoring "/" inside a {token}. */
export function splitPath(tpl: string): { dirs: string; file: string } {
  let depth = 0;
  let cut = -1;
  for (let i = 0; i < tpl.length; i++) {
    const c = tpl[i];
    if (c === "{" && tpl[i + 1] === "{") i++;
    else if (c === "{") depth++;
    else if (c === "}") depth = Math.max(0, depth - 1);
    else if (c === "/" && depth === 0) cut = i;
  }
  return cut === -1 ? { dirs: "", file: tpl } : { dirs: tpl.slice(0, cut), file: tpl.slice(cut + 1) };
}

/**
 * Put `from`'s path onto `to`. With `keepName` the destination keeps its own file
 * name — which is usually what you want, since the renders end in .exr / .jpg.
 */
export function copyPath(from: string, to: string, keepName: boolean): string {
  if (!keepName) return from;
  const a = splitPath(from);
  const b = splitPath(to);
  return a.dirs ? `${a.dirs}/${b.file}` : b.file;
}
