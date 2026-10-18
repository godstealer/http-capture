import type { RequestDraft, Scripts } from "./types";
import { encodeText } from "./types";
export interface SavedRequestContext {
  grpcDraft?: { method: string; json: string };
  requestMode?: 'http' | 'grpc' | 'websocket';
}
export interface Library {
  revision: number;
  collections: { id: string; name: string }[];
  requests: {
    id: string;
    name: string;
    collectionId: string;
    draft: RequestDraft;
    grpcDraft?: SavedRequestContext['grpcDraft'];
    requestMode?: SavedRequestContext['requestMode'];
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
export interface SaveRequestSelection {
  name: string;
  collectionId?: string;
  collectionName?: string;
}

// Build from the complete local library, including edits on the collection page.
// The caller only replaces UI state after the revision-checked save succeeds.
export function withSavedRequest(
  library: Library,
  draft: RequestDraft,
  selection: SaveRequestSelection,
  requestId?: string,
  context: SavedRequestContext = {},
): { library: Library; requestId: string; collectionName: string } {
  const name = selection.name.trim();
  if (!name || new TextEncoder().encode(name).length > 512)
    throw new Error('Invalid request name');
  const next = structuredClone(library);
  let collection = next.collections.find(c => c.id === selection.collectionId);
  if (selection.collectionId && !collection) throw new Error('Collection not found');
  if (!collection) {
    const collectionName = selection.collectionName?.trim() ?? '';
    if (!collectionName || new TextEncoder().encode(collectionName).length > 512)
      throw new Error('Invalid collection name');
    collection = { id: crypto.randomUUID(), name: collectionName };
    next.collections.push(collection);
  }
  const index = requestId ? next.requests.findIndex(r => r.id === requestId) : -1;
  // A removed/imported-away request must be saved explicitly as a new request.
  if (requestId && index < 0) throw new Error('Saved request not found');
  const id = requestId ?? crypto.randomUUID();
  const request = { ...(index >= 0 ? next.requests[index] : {}), id, name, collectionId: collection.id, draft: structuredClone(draft), ...structuredClone(context) };
  if (index >= 0) next.requests[index] = request;
  else next.requests.push(request);
  return { library: next, requestId: id, collectionName: collection.name };
}

export function suggestedRequestName(draft: RequestDraft): string {
  // Do not cut a surrogate pair when a long path contains emoji.
  const shorten = (name: string) => name.slice(0, 128).replace(/[\uD800-\uDBFF]$/, '');
  try {
    const url = new URL(draft.url);
    return shorten(`${draft.method} ${url.host}${url.pathname === '/' ? '' : url.pathname}`);
  } catch {
    return shorten(`${draft.method} ${draft.url}`.trim());
  }
}
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
  draft.headers = draft.headers.map((h) => h.disabled ? h : ({
    ...h,
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
      (item.requestMode !== undefined && !['http', 'grpc', 'websocket'].includes(item.requestMode)) ||
      (item.grpcDraft !== undefined && (!object(item.grpcDraft) || typeof item.grpcDraft.method !== 'string' || typeof item.grpcDraft.json !== 'string')) ||
      !object(r) ||
      typeof r.method !== "string" ||
      typeof r.url !== "string" ||
      typeof r.bodyBase64 !== "string" ||
      !Array.isArray(r.headers) ||
      r.headers.some(
        (h: any) =>
          !object(h) ||
          typeof h.name !== "string" ||
          typeof h.value !== "string" ||
          (h.disabled !== undefined && typeof h.disabled !== "boolean"),
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
