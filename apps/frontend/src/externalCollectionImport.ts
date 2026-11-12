import { importCurl } from './importCurl';
import { parseLibrary, suggestedRequestName, type Library } from './library';
import { parseOpenApi, parseInsomnia, parseHarCollection } from './externalImportOther';
import { parsePostman, parseHoppscotch } from './externalImportPostmanHoppscotch';
import type { ExternalImportFormat, ExternalImportPlan } from './externalImportTypes';

const MAX_IMPORT_BYTES = 8 * 1024 * 1024;
const MAX_ITEMS = 10_000;
const MAX_NAME_BYTES = 512;
const encoder = new TextEncoder();

function name(value: unknown, kind: string): string {
  if (typeof value !== 'string' || !value.trim() || encoder.encode(value).byteLength > MAX_NAME_BYTES)
    throw new Error(`${kind}名称不能为空，且不能超过 512 字节。`);
  return value;
}

function freshId(ids: Set<string>): string {
  let id: string;
  do { id = crypto.randomUUID(); } while (ids.has(id));
  ids.add(id);
  return id;
}

/**
 * Append a reviewed external import without changing the current library or
 * the import preview. IDs from foreign tools are never copied into our store.
 * Persistence and revision conflict handling remain with the library page.
 */
export function mergeExternalPlan(library: Library, plan: ExternalImportPlan): Library {
  if (!plan || !Array.isArray(plan.collections) || !Array.isArray(plan.warnings) ||
      (plan.environments !== undefined && !Array.isArray(plan.environments)))
    throw new Error('导入预览无效。');
  const collectionCount = plan.collections.length;
  const environmentCount = plan.environments?.length ?? 0;
  const requestCount = plan.collections.reduce((count, collection) => {
    if (!collection || !Array.isArray(collection.requests)) throw new Error('导入集合无效。');
    return count + collection.requests.length;
  }, 0);
  if (!collectionCount && !environmentCount) throw new Error('文件中没有可导入的集合或环境。');
  if (collectionCount + environmentCount + requestCount > MAX_ITEMS)
    throw new Error('导入项目超过 10000 项，请分批导入。');

  const next = structuredClone(library);
  const ids = new Set([...next.collections, ...next.requests, ...next.environments, ...next.scripts].map(item => item.id));
  let bodyBytes = 0;
  for (const collection of plan.collections) {
    const collectionId = freshId(ids);
    next.collections.push({ id: collectionId, name: name(collection.name, '集合') });
    for (const request of collection.requests) {
      if (!request || !request.draft || typeof request.draft.bodyBase64 !== 'string')
        throw new Error('导入请求无效。');
      let bytes: number;
      try { bytes = atob(request.draft.bodyBase64).length; }
      catch { throw new Error('导入请求正文的 Base64 无效。'); }
      bodyBytes += bytes;
      if (bodyBytes > MAX_IMPORT_BYTES) throw new Error('导入请求正文总量不能超过 8 MiB，请分批导入。');
      next.requests.push({
        id: freshId(ids),
        name: name(request.name, '请求'),
        collectionId,
        draft: structuredClone(request.draft),
      });
    }
  }
  for (const environment of plan.environments ?? []) {
    if (!environment || !environment.variables || Array.isArray(environment.variables) ||
        typeof environment.variables !== 'object' ||
        Object.values(environment.variables).some(value => typeof value !== 'string'))
      throw new Error('导入环境变量无效。');
    next.environments.push({
      id: freshId(ids),
      name: name(environment.name, '环境'),
      variables: structuredClone(environment.variables),
    });
  }
  const serialized = JSON.stringify(next);
  if (encoder.encode(serialized).byteLength > MAX_IMPORT_BYTES)
    throw new Error('导入后集合数据超过 8 MiB，请拆分或精简集合后重试。');
  // Reuse the strict application-file validator before exposing any imported
  // draft to the editor or submitting it to the revision-checked backend.
  return parseLibrary(JSON.stringify({ format: 'http-capture-library', version: 1, library: next }), library.revision);
}

export function parseExternalImport(text: string, format: ExternalImportFormat): ExternalImportPlan {
  if (encoder.encode(text).byteLength > MAX_IMPORT_BYTES)
    throw new Error('导入文件不能超过 8 MiB，请拆分文件后重试。');
  switch (format) {
    case 'postman': return parsePostman(text);
    case 'hoppscotch': return parseHoppscotch(text);
    case 'openapi': return parseOpenApi(text);
    case 'insomnia': return parseInsomnia(text);
    case 'har': return parseHarCollection(text);
    case 'curl': {
      const draft = importCurl(text);
      return { source: 'curl', collections: [{ name: 'cURL 导入', requests: [{ name: suggestedRequestName(draft), draft }] }], warnings: [] };
    }
    default: throw new Error('不支持的导入格式。');
  }
}
