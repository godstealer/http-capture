import type { RequestDraft, Scripts } from "./types";
import { encodeText } from "./types";
export interface Library {
  revision: number;
  collections: { id: string; name: string }[];
  requests: {
    id: string;
    name: string;
    collectionId: string;
    draft: RequestDraft;
  }[];
  environments: {
    id: string;
    name: string;
    variables: Record<string, string>;
  }[];
  scripts: { id: string; name: string; scripts: Scripts }[];
}
export const emptyLibrary: Library = {
  revision: 0,
  collections: [],
  requests: [],
  environments: [],
  scripts: [],
};
export function resolveEnvironment(
  request: RequestDraft,
  variables: Record<string, string>,
): RequestDraft {
  const draft = structuredClone(request);
  const scope = { ...variables, ...draft.scripts?.variables };
  const replace = (text: string) =>
    text.replace(/\{\{\s*([^{}]+?)\s*\}\}/g, (_, name: string) => {
      if (!Object.prototype.hasOwnProperty.call(scope, name))
        throw new Error(`Undefined variable: ${name}`);
      return scope[name];
    });
  draft.url = replace(draft.url);
  draft.headers = draft.headers.map((h) => ({
    name: replace(h.name),
    value: replace(h.value),
  }));
  let body: string | undefined;
  try {
    body = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
      Uint8Array.from(atob(draft.bodyBase64), (c) => c.charCodeAt(0)),
    );
  } catch {
    /* Binary bodies are never interpolated. */
  }
  if (body !== undefined) draft.bodyBase64 = encodeText(replace(body));
  if (draft.scripts) draft.scripts.variables = scope;
  return draft;
}
export function parseLibrary(text: string, revision: number): Library {
  const input = JSON.parse(text);
  if (
    input?.format !== "http-capture-library" ||
    input.version !== 1 ||
    !input.library
  )
    throw new Error("Invalid library file");
  const data = input.library;
  if (
    !["collections", "requests", "environments", "scripts"].every((k) =>
      Array.isArray(data[k]),
    )
  )
    throw new Error("Invalid library lists");
  const object = (v: any) =>
    v !== null && typeof v === "object" && !Array.isArray(v);
  const strings = (v: any) =>
    object(v) && Object.values(v).every((x) => typeof x === "string");
  const ids = new Set<string>();
  for (const item of [
    ...data.collections,
    ...data.requests,
    ...data.environments,
    ...data.scripts,
  ]) {
    if (
      !object(item) ||
      typeof item.id !== "string" ||
      !item.id ||
      item.id.length > 128 ||
      ids.has(item.id) ||
      typeof item.name !== "string" ||
      !item.name.trim() ||
      item.name.length > 512
    )
      throw Error("Invalid library item or duplicate ID");
    ids.add(item.id);
  }
  const scripts = (v: any) =>
    object(v) &&
    typeof v.enabled === "boolean" &&
    typeof v.before === "string" &&
    typeof v.after === "string" &&
    strings(v.variables) &&
    (v.modules === undefined ||
      (object(v.modules) &&
        ["encoding", "crypto", "utils"].every(
          (k) => typeof v.modules[k] === "boolean",
        )));
  for (const item of data.environments)
    if (!strings(item.variables)) throw Error("Invalid environment variables");
  for (const item of data.scripts)
    if (!scripts(item.scripts)) throw Error("Invalid script template");
  for (const item of data.requests) {
    const r = item.draft;
    if (
      !data.collections.some((c: any) => c.id === item.collectionId) ||
      !object(r) ||
      typeof r.method !== "string" ||
      typeof r.url !== "string" ||
      typeof r.bodyBase64 !== "string" ||
      !Array.isArray(r.headers) ||
      r.headers.some(
        (h: any) =>
          !object(h) ||
          typeof h.name !== "string" ||
          typeof h.value !== "string",
      ) ||
      !object(r.tls) ||
      typeof r.tls.preset !== "string" ||
      (r.scripts !== undefined && !scripts(r.scripts))
    )
      throw Error("Invalid saved request");
    atob(r.bodyBase64);
  }
  // File imports are explicit local edits; CAS revision always belongs to the destination.
  return { ...data, revision };
}
