import { encodeText, type Header, type RequestDraft } from './types';
import type { ExternalImportPlan, ImportedRequest } from './externalImportTypes';

type RecordValue = Record<string, unknown>;
const object = (value: unknown): value is RecordValue =>
  value !== null && typeof value === 'object' && !Array.isArray(value);
const list = (value: unknown): unknown[] => Array.isArray(value) ? value : [];
const str = (value: unknown): string => typeof value === 'string' ? value : '';

function readObject(text: string): RecordValue {
  let value: unknown;
  try { value = JSON.parse(text); } catch { throw new Error('文件不是有效的 JSON。'); }
  if (!object(value)) throw new Error('集合文件必须是 JSON 对象。');
  return value;
}

function name(value: unknown, fallback: string): string {
  const source = str(value).trim() || fallback;
  let result = '';
  for (const codepoint of source) {
    if (new TextEncoder().encode(result + codepoint).byteLength > 512) break;
    result += codepoint;
  }
  return result;
}

function draft(method: unknown, url: string, headers: Header[], body = ''): RequestDraft {
  return { engine: 'auto', method: str(method).toUpperCase() || 'GET', url,
    headers, bodyBase64: encodeText(body), tls: { preset: 'native' } };
}

function hasHeader(headers: Header[], key: string): boolean {
  return headers.some(header => !header.disabled && header.name.toLowerCase() === key.toLowerCase());
}
function headerUnlessPresent(headers: Header[], key: string, value: string): void {
  if (!hasHeader(headers, key)) headers.push({ name: key, value });
}
function queryPart(value: string): string {
  // Environment placeholders must remain usable after URL encoding.
  return value.split(/(\{\{[^{}]+\}\})/g)
    .map(part => part.startsWith('{{') && part.endsWith('}}') ? part : encodeURIComponent(part))
    .join('');
}
function appendQuery(url: string, values: { key: string; value: string }[]): string {
  if (!values.length) return url;
  const fragment = url.indexOf('#');
  const base = fragment < 0 ? url : url.slice(0, fragment);
  const hash = fragment < 0 ? '' : url.slice(fragment);
  return `${base}${base.includes('?') ? '&' : '?'}${values.map(item =>
    `${queryPart(item.key)}=${queryPart(item.value)}`).join('&')}${hash}`;
}
function mergeHoppscotchParams(url: string, params: { key: string; value: string; active: boolean }[]): string {
  if (!params.length) return url;
  const hashAt = url.indexOf('#');
  const hash = hashAt < 0 ? '' : url.slice(hashAt);
  const withoutHash = hashAt < 0 ? url : url.slice(0, hashAt);
  const queryAt = withoutHash.indexOf('?');
  const base = queryAt < 0 ? withoutHash : withoutHash.slice(0, queryAt);
  const rawPairs = queryAt < 0 ? [] : withoutHash.slice(queryAt + 1).split('&').filter(Boolean);
  const decode = (value: string): string => {
    try { return decodeURIComponent(value.replace(/\+/g, ' ')); } catch { return value; }
  };
  const pairs = rawPairs.filter(pair => {
    const at = pair.indexOf('=');
    const key = decode(at < 0 ? pair : pair.slice(0, at));
    const value = decode(at < 0 ? '' : pair.slice(at + 1));
    return !params.some(param => !param.active && param.key === key && param.value === value);
  });
  for (const param of params.filter(param => param.active)) {
    const encoded = `${queryPart(param.key)}=${queryPart(param.value)}`;
    const present = pairs.some(pair => {
      const at = pair.indexOf('=');
      return decode(at < 0 ? pair : pair.slice(0, at)) === param.key &&
        decode(at < 0 ? '' : pair.slice(at + 1)) === param.value;
    });
    if (!present) pairs.push(encoded);
  }
  return `${base}${pairs.length ? `?${pairs.join('&')}` : ''}${hash}`;
}
function basicHeader(username: string, password: string): string {
  return `Basic ${encodeText(`${username}:${password}`)}`;
}
function isVariable(value: string): boolean { return /\{\{[^{}]+\}\}/.test(value); }

function applyPostmanAuth(auth: unknown, current: RequestDraft, label: string, warnings: string[]): void {
  if (!object(auth)) return;
  const kind = str(auth.type);
  if (!kind || kind === 'noauth') return;
  if (kind === 'bearer') {
    const token = str(list(auth.bearer).filter(object).find(item => item.key === 'token')?.value);
    if (token) headerUnlessPresent(current.headers, 'Authorization', `Bearer ${token}`);
    else warnings.push(`${label}: Bearer token 为空，未生成认证头。`);
  } else if (kind === 'basic') {
    const fields = list(auth.basic).filter(object);
    const user = str(fields.find(item => item.key === 'username')?.value);
    const pass = str(fields.find(item => item.key === 'password')?.value);
    if (isVariable(user) || isVariable(pass)) warnings.push(`${label}: Basic Auth 含变量，无法在发送前正确编码，请手动配置。`);
    else headerUnlessPresent(current.headers, 'Authorization', basicHeader(user, pass));
  } else if (kind === 'apikey') {
    const fields = list(auth.apikey).filter(object);
    const key = str(fields.find(item => item.key === 'key')?.value);
    const value = str(fields.find(item => item.key === 'value')?.value);
    const location = str(fields.find(item => item.key === 'in')?.value).toLowerCase();
    if (!key) warnings.push(`${label}: API Key 缺少名称，请手动配置。`);
    else if (location === 'query') current.url = appendQuery(current.url, [{ key, value }]);
    else headerUnlessPresent(current.headers, key, value);
  } else warnings.push(`${label}: ${kind} 认证需要手动配置。`);
}

function multipart(fields: { key: string; value: string; contentType?: string }[], headers: Header[]): string {
  const boundary = `----http-capture-${crypto.randomUUID()}`;
  const escapeName = (value: string) => value.replace(/[\r\n]/g, ' ').replace(/[\\"]/g, '\\$&');
  const body = fields.map(field => `--${boundary}\r\nContent-Disposition: form-data; name="${escapeName(field.key)}"\r\n${field.contentType ? `Content-Type: ${field.contentType}\r\n` : ''}\r\n${field.value}\r\n`).join('') + `--${boundary}--\r\n`;
  const existing = headers.find(header => !header.disabled && header.name.toLowerCase() === 'content-type');
  if (existing) existing.value = `multipart/form-data; boundary=${boundary}`;
  else headers.push({ name: 'Content-Type', value: `multipart/form-data; boundary=${boundary}` });
  for (let index = headers.length - 1; index >= 0; index--) {
    if (headers[index].name.toLowerCase() === 'content-length') headers.splice(index, 1);
  }
  return body;
}

function postmanBody(value: unknown, headers: Header[], label: string, warnings: string[]): string | null {
  if (value === null || value === undefined) return '';
  if (!object(value)) { warnings.push(`${label}: 正文结构无效，已跳过请求。`); return null; }
  if (value.disabled === true) return '';
  const mode = str(value.mode);
  if (mode === 'raw') {
    if (typeof value.raw !== 'string') { warnings.push(`${label}: raw 正文缺失，已跳过请求。`); return null; }
    const options = object(value.options) && object(value.options.raw) ? value.options.raw : null;
    const language = options ? str(options.language) : '';
    if (language === 'json') headerUnlessPresent(headers, 'Content-Type', 'application/json');
    return value.raw;
  }
  if (mode === 'urlencoded') {
    if (!Array.isArray(value.urlencoded)) { warnings.push(`${label}: URL 编码正文缺失，已跳过请求。`); return null; }
    headerUnlessPresent(headers, 'Content-Type', 'application/x-www-form-urlencoded');
    for (let index = headers.length - 1; index >= 0; index--) {
      if (headers[index].name.toLowerCase() === 'content-length') headers.splice(index, 1);
    }
    return value.urlencoded.filter(object).filter(item => item.disabled !== true)
      .map(item => `${queryPart(str(item.key))}=${queryPart(str(item.value))}`).join('&');
  }
  if (mode === 'formdata') {
    if (!Array.isArray(value.formdata)) { warnings.push(`${label}: 表单正文缺失，已跳过请求。`); return null; }
    const fields = value.formdata.filter(object).filter(item => item.disabled !== true);
    if (fields.some(item => item.type === 'file' || item.src !== undefined || typeof item.value !== 'string')) {
      warnings.push(`${label}: 表单包含外部文件或无法读取的字段，已跳过请求以免丢失正文。`); return null;
    }
    return multipart(fields.map(item => ({ key: str(item.key), value: str(item.value), contentType: str(item.contentType) || undefined })), headers);
  }
  if (mode === 'graphql') {
    if (!object(value.graphql) || typeof value.graphql.query !== 'string') {
      warnings.push(`${label}: GraphQL 正文无效，已跳过请求。`); return null;
    }
    let variables: unknown = {};
    try { variables = JSON.parse(str(value.graphql.variables) || '{}'); }
    catch { warnings.push(`${label}: GraphQL 变量不是有效 JSON，已跳过请求。`); return null; }
    headerUnlessPresent(headers, 'Content-Type', 'application/json');
    return JSON.stringify({ query: value.graphql.query, variables });
  }
  if (mode === 'file' && object(value.file) && typeof value.file.content === 'string') {
    // Exported inline content is text; a file path alone cannot supply bytes.
    return value.file.content;
  }
  warnings.push(`${label}: ${mode || '未知'} 正文需要外部文件或暂不支持，已跳过请求。`);
  return null;
}

function postmanUrl(value: unknown): string {
  if (typeof value === 'string') return value;
  if (!object(value)) return '';
  const raw = str(value.raw);
  const query = list(value.query).filter(object);
  if (raw) {
    if (!query.some(item => item.disabled === true)) return raw;
    // Query metadata is authoritative when a disabled row is present.
    const base = raw.split('?')[0];
    return appendQuery(base, query.filter(item => item.disabled !== true).map(item => ({ key: str(item.key), value: str(item.value) })));
  }
  const protocol = str(value.protocol) || 'https';
  const host = Array.isArray(value.host) ? value.host.map(str).join('.') : str(value.host);
  const path = Array.isArray(value.path) ? value.path.map(item => object(item) ? str(item.value) : str(item)).join('/') : str(value.path);
  const port = str(value.port);
  const url = `${protocol}://${host}${port ? `:${port}` : ''}/${path.replace(/^\//, '')}`;
  return appendQuery(url, query.filter(item => item.disabled !== true).map(item => ({ key: str(item.key), value: str(item.value) })));
}
function postmanHeaders(value: unknown, label: string, warnings: string[]): Header[] {
  if (Array.isArray(value)) return value.filter(object)
    .map(header => ({ name: str(header.key), value: str(header.value), disabled: header.disabled === true }))
    .filter(header => header.name);
  if (typeof value !== 'string') return [];
  const headers: Header[] = [];
  for (const line of value.split(/\r?\n/).filter(Boolean)) {
    const at = line.indexOf(':');
    if (at < 1) { warnings.push(`${label}: 无效的请求头行已跳过。`); continue; }
    headers.push({ name: line.slice(0, at).trim(), value: line.slice(at + 1).trimStart() });
  }
  return headers;
}

export function parsePostman(text: string): ExternalImportPlan {
  const data = readObject(text);
  const info = object(data.info) ? data.info : null;
  const schema = str(info?.schema ?? info?._postman_schema);
  if (!info || !Array.isArray(data.item) || !/(?:schema\.)?(?:getpostman|postman)\.com.*(?:v|#)2\.[01](?:\.0)?/i.test(schema))
    throw new Error('不是支持的 Postman Collection v2.0/v2.1 文件。');
  const root = name(info.name, 'Postman 集合');
  const plan: ExternalImportPlan = { source: 'Postman Collection', collections: [], warnings: [] };
  const variables: Record<string, string> = {};
  for (const entry of list(data.variable).filter(object)) {
    const key = str(entry.key || entry.id);
    if (key && entry.disabled !== true && entry.value !== undefined && entry.value !== null)
      variables[key] = String(entry.value);
  }
  if (Object.keys(variables).length) plan.environments = [{ name: root, variables }];
  let count = 0;
  const walk = (items: unknown[], path: string[], inheritedAuth: unknown): void => {
    const collection = { name: name(path.join(' / '), root), requests: [] as ImportedRequest[] };
    for (const item of items) {
      if (!object(item)) { plan.warnings.push(`${collection.name}: 无效条目已跳过。`); continue; }
      const label = name(item.name, '未命名请求');
      if (Array.isArray(item.item)) {
        if (list(item.event).length) plan.warnings.push(`${collection.name} / ${label}: 文件夹级 Postman 脚本未导入。`);
        walk(item.item, [...path, label], item.auth === undefined ? inheritedAuth : item.auth);
        continue;
      }
      if (++count > 10_000) throw new Error('Postman 文件超过 10000 个请求。');
      const request = typeof item.request === 'string' ? { url: item.request, method: 'GET' } : item.request;
      if (!object(request)) { plan.warnings.push(`${collection.name} / ${label}: 请求无效，已跳过。`); continue; }
      const url = postmanUrl(request.url);
      if (!url) { plan.warnings.push(`${collection.name} / ${label}: 缺少 URL，已跳过。`); continue; }
      const headers = postmanHeaders(request.header, `${collection.name} / ${label}`, plan.warnings);
      const body = postmanBody(request.body, headers, `${collection.name} / ${label}`, plan.warnings);
      if (body === null) continue;
      const current = draft(request.method, url, headers, body);
      applyPostmanAuth(request.auth === undefined ? inheritedAuth : request.auth, current, `${collection.name} / ${label}`, plan.warnings);
      if (list(item.event).length) plan.warnings.push(`${collection.name} / ${label}: Postman 脚本未导入。`);
      collection.requests.push({ name: label, draft: current });
    }
    if (collection.requests.length || (path.length === 1 && !plan.collections.length)) plan.collections.push(collection);
  };
  walk(data.item, [root], data.auth);
  if (list(data.event).length) plan.warnings.push(`${root}: 集合级 Postman 脚本未导入。`);
  if (!plan.collections.length && !plan.environments?.length) throw new Error('Postman 文件中没有可导入的 HTTP 请求。');
  return plan;
}

function hoppscotchBody(value: unknown, headers: Header[], label: string, warnings: string[]): string | null {
  if (value === undefined || value === null) return '';
  if (!object(value)) { warnings.push(`${label}: 正文结构无效，已跳过请求。`); return null; }
  if (value.contentType === null) return '';
  const contentType = str(value.contentType);
  if (contentType === 'multipart/form-data') {
    if (!Array.isArray(value.body)) { warnings.push(`${label}: 表单正文无效，已跳过请求。`); return null; }
    const fields = value.body.filter(object).filter(item => item.active !== false);
    if (fields.some(item => item.isFile === true || typeof item.value !== 'string')) {
      warnings.push(`${label}: 表单包含文件或无法读取的字段，已跳过请求以免丢失正文。`); return null;
    }
    return multipart(fields.map(item => ({ key: str(item.key), value: str(item.value), contentType: str(item.contentType) || undefined })), headers);
  }
  if (contentType === 'application/octet-stream' || (contentType === 'binary' && typeof value.body !== 'string')) {
    warnings.push(`${label}: 二进制正文无法从 JSON 导出文件还原，已跳过请求。`); return null;
  }
  if (typeof value.body !== 'string') { warnings.push(`${label}: 正文无效，已跳过请求。`); return null; }
  if (contentType && contentType !== 'binary') headerUnlessPresent(headers, 'Content-Type', contentType);
  return value.body;
}
function hoppscotchAuth(auth: unknown, current: RequestDraft, label: string, warnings: string[]): void {
  if (!object(auth) || auth.authActive === false) return;
  const kind = str(auth.authType);
  if (kind === 'inherit' || kind === 'none' || !kind) return;
  if (kind === 'bearer') {
    if (typeof auth.token === 'string') headerUnlessPresent(current.headers, 'Authorization', `Bearer ${auth.token}`);
    else warnings.push(`${label}: Bearer token 缺失。`);
  } else if (kind === 'basic') {
    const user = str(auth.username), pass = str(auth.password);
    if (isVariable(user) || isVariable(pass)) warnings.push(`${label}: Basic Auth 含变量，请手动配置。`);
    else headerUnlessPresent(current.headers, 'Authorization', basicHeader(user, pass));
  } else if (kind === 'api-key') {
    const key = str(auth.key), value = str(auth.value);
    if (!key) warnings.push(`${label}: API Key 缺少名称。`);
    else if (['QUERY_PARAMS', 'Query params'].includes(str(auth.addTo))) current.url = appendQuery(current.url, [{ key, value }]);
    else headerUnlessPresent(current.headers, key, value);
  } else warnings.push(`${label}: ${kind} 认证需要手动配置。`);
}

export function parseHoppscotch(text: string): ExternalImportPlan {
  let data: unknown;
  try { data = JSON.parse(text); } catch { throw new Error('文件不是有效的 JSON。'); }
  if (!object(data) && !Array.isArray(data)) throw new Error('不是 Hoppscotch 集合文件。');
  const roots = Array.isArray(data) ? data : Array.isArray(data.collections) ? data.collections : [data];
  const valid = roots.filter(object).filter(item => typeof item.name === 'string' && Array.isArray(item.folders) && Array.isArray(item.requests));
  if (!valid.length) throw new Error('不是 Hoppscotch 集合文件。');
  const plan: ExternalImportPlan = { source: 'Hoppscotch', collections: [], warnings: [] };
  let count = 0;
  const walk = (node: RecordValue, path: string[], parentHeaders: Header[], parentAuth: unknown, parentVariables: Record<string, string>): void => {
    const label = name(node.name, '未命名集合');
    const fullPath = [...path, label];
    const ownHeaders: Header[] = list(node.headers).filter(object).map(item => ({ name: str(item.key), value: str(item.value), disabled: item.active === false })).filter(item => item.name);
    const names = new Set(ownHeaders.map(item => item.name.toLowerCase()));
    const inheritedHeaders = [...parentHeaders.filter(item => !names.has(item.name.toLowerCase())), ...ownHeaders];
    const ownAuth = object(node.auth) && node.auth.authType !== 'inherit' ? node.auth : parentAuth;
    const variables = { ...parentVariables };
    for (const item of list(node.variables).filter(object)) {
      const key = str(item.key);
      if (key) variables[key] = str(item.currentValue ?? item.initialValue ?? item.value);
    }
    const collection = { name: name(fullPath.join(' / '), label), requests: [] as ImportedRequest[] };
    for (const entry of list(node.requests)) {
      if (++count > 10_000) throw new Error('Hoppscotch 文件超过 10000 个请求。');
      if (!object(entry)) { plan.warnings.push(`${collection.name}: 无效请求已跳过。`); continue; }
      const requestName = name(entry.name, '未命名请求');
      const requestLabel = `${collection.name} / ${requestName}`;
      if (entry.type && entry.type !== 'rest') { plan.warnings.push(`${requestLabel}: ${entry.type} 请求暂不支持，已跳过。`); continue; }
      const endpoint = str(entry.endpoint);
      if (!endpoint) { plan.warnings.push(`${requestLabel}: 缺少 endpoint，已跳过。`); continue; }
      const localHeaders: Header[] = list(entry.headers).filter(object).map(item => ({ name: str(item.key), value: str(item.value), disabled: item.active === false })).filter(item => item.name);
      const localNames = new Set(localHeaders.map(item => item.name.toLowerCase()));
      const headers = [...inheritedHeaders.filter(item => !localNames.has(item.name.toLowerCase())), ...localHeaders];
      const body = hoppscotchBody(entry.body, headers, requestLabel, plan.warnings);
      if (body === null) continue;
      const params = list(entry.params).filter(object).filter(item => str(item.key))
        .map(item => ({ key: str(item.key), value: str(item.value), active: item.active !== false }));
      const current = draft(entry.method, mergeHoppscotchParams(endpoint, params), headers, body);
      const auth = object(entry.auth) && entry.auth.authType !== 'inherit' ? entry.auth : ownAuth;
      hoppscotchAuth(auth, current, requestLabel, plan.warnings);
      if (str(entry.preRequestScript) || str(entry.testScript)) plan.warnings.push(`${requestLabel}: Hoppscotch 脚本未导入。`);
      collection.requests.push({ name: requestName, draft: current });
    }
    if (collection.requests.length || (!path.length && !list(node.folders).length)) plan.collections.push(collection);
    for (const folder of list(node.folders)) if (object(folder)) walk(folder, fullPath, inheritedHeaders, ownAuth, variables);
    if (str(node.preRequestScript) || str(node.testScript)) plan.warnings.push(`${collection.name}: 集合级脚本未导入。`);
    if (list(node.variables).length) {
      if (Object.keys(variables).length) (plan.environments ??= []).push({ name: collection.name, variables });
    }
  };
  for (const root of valid) walk(root, [], [], undefined, {});
  if (!plan.collections.length && !plan.environments?.length) throw new Error('Hoppscotch 文件中没有可导入的 HTTP 请求。');
  return plan;
}
