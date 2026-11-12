import { encodeText, type Header, type RequestDraft } from './types';
import type { ExternalImportPlan, ImportedRequest } from './externalImportTypes';

type ObjectValue = Record<string, unknown>;
const object = (value: unknown): ObjectValue => value && typeof value === 'object' && !Array.isArray(value) ? value as ObjectValue : {};
const array = (value: unknown): unknown[] => Array.isArray(value) ? value : [];
const string = (value: unknown): string => typeof value === 'string' ? value : '';
function bounded(value: string): string {
  let result = '';
  for (const character of value.trim()) {
    if (new TextEncoder().encode(result + character).byteLength > 512) break;
    result += character;
  }
  return result;
}
const draft = (method: string, url: string, headers: Header[], body = ''): RequestDraft => ({
  method: method.toUpperCase(), url, headers, bodyBase64: encodeText(body), engine: 'auto', tls: { preset: 'native' },
});
function json(text: string, name: string): ObjectValue {
  try {
    const parsed: unknown = JSON.parse(text.replace(/^\uFEFF/, ''));
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) return parsed as ObjectValue;
  } catch { /* Show the format-specific error below. */ }
  throw new Error(`${name} 需要有效的 JSON 对象。`);
}
function headers(value: unknown): Header[] {
  return array(value).flatMap(item => {
    const row = object(item); const name = string(row.name);
    return name ? [{ name, value: string(row.value), disabled: row.disabled === true }] : [];
  });
}
function putHeader(list: Header[], name: string, value: string) {
  if (value && !list.some(row => row.name.toLowerCase() === name.toLowerCase())) list.push({ name, value });
}
function warn(warnings: string[], message: string) {
  if (!warnings.includes(message)) warnings.push(message);
}
function named(name: string, method: string, url: string): string {
  return bounded(name || `${method.toUpperCase()} ${url}`) || '未命名请求';
}
function appendQuery(url: string, params: { name: string; value: string }[]): string {
  if (!params.length) return url;
  const fragmentAt = url.indexOf('#');
  const target = fragmentAt < 0 ? url : url.slice(0, fragmentAt);
  const fragment = fragmentAt < 0 ? '' : url.slice(fragmentAt);
  const suffix = params.map(({ name, value }) => `${encodeURIComponent(name)}=${encodeURIComponent(value).replace(/%7B%7B/g, '{{').replace(/%7D%7D/g, '}}')}`).join('&');
  return `${target}${target.includes('?') ? '&' : '?'}${suffix}${fragment}`;
}

/** OpenAPI 3.0/3.1 JSON. Specifications describe request shapes, not runnable examples. */
export function parseOpenApi(text: string): ExternalImportPlan {
  const root = json(text, 'OpenAPI');
  if (!/^3\.(?:0|1)(?:\.|$)/.test(string(root.openapi))) throw new Error('仅支持 OpenAPI 3.0/3.1 JSON；Swagger 2.0 和 YAML 尚未支持。');
  const title = bounded(string(object(root.info).title)) || 'OpenAPI 导入';
  const warnings: string[] = [];
  const requests: ImportedRequest[] = [];
  const paths = object(root.paths);
  const resolve = (value: unknown): ObjectValue => {
    let item = object(value);
    const seen = new Set<string>();
    while (typeof item.$ref === 'string') {
      const ref = item.$ref;
      if (!ref.startsWith('#/') || seen.has(ref)) { warn(warnings, `无法解析引用 ${ref}。`); return {}; }
      seen.add(ref);
      let current: unknown = root;
      for (const part of ref.slice(2).split('/').map(segment => segment.replace(/~1/g, '/').replace(/~0/g, '~'))) current = object(current)[part];
      if (!current) { warn(warnings, `无法解析引用 ${ref}。`); return {}; }
      item = object(current);
    }
    return item;
  };
  const methods = ['get', 'post', 'put', 'patch', 'delete', 'head', 'options', 'trace'];
  for (const [path, rawPath] of Object.entries(paths)) {
    const pathItem = resolve(rawPath);
    for (const method of methods) {
      if (!pathItem[method]) continue;
      const operation = object(pathItem[method]);
      const availableServers = array(operation.servers).length ? array(operation.servers) : array(pathItem.servers).length ? array(pathItem.servers) : array(root.servers);
      if (availableServers.length > 1) warn(warnings, '多个服务器地址只导入第一个，请核对所需环境。');
      const server = object(availableServers[0]);
      let base = string(server.url);
      if (!base) { base = '{{baseUrl}}'; warn(warnings, '规范未提供服务器地址，请发送前填写 baseUrl。'); }
      const serverVariables = object(server.variables);
      base = base.replace(/(?<!\{)\{([^{}]+)\}(?!\})/g, (_match, key: string) => {
        const value = string(object(serverVariables[key]).default);
        if (!value) warn(warnings, `服务器变量 ${key} 没有默认值，请检查 URL。`);
        return value || `{{${key}}}`;
      });
      if (!/^https?:\/\//i.test(base) && !base.startsWith('{{')) warn(warnings, `相对服务器地址 ${base} 需要在发送前改为绝对 URL。`);
      let url = `${base.replace(/\/$/, '')}${path}`;
      const requestHeaders: Header[] = [];
      const declared = [...array(pathItem.parameters), ...array(operation.parameters)];
      const parameters = new Map<string, ObjectValue>();
      for (const raw of declared) {
        const parameter = resolve(raw);
        const name = string(parameter.name); const where = string(parameter.in);
        if (name && where) parameters.set(`${where}:${name}`, parameter);
      }
      const query: { name: string; value: string }[] = [];
      const cookie: string[] = [];
      for (const parameter of parameters.values()) {
        const name = string(parameter.name); const where = string(parameter.in);
        const schema = resolve(parameter.schema);
        const example = parameter.example ?? schema.example ?? schema.default;
        const value = typeof example === 'string' || typeof example === 'number' || typeof example === 'boolean' ? String(example) : `{{${name}}}`;
        if (example === undefined) warn(warnings, '部分参数没有示例值，使用 {{参数名}} 占位；发送前请填写。');
        if (where === 'path') url = url.replace(`{${name}}`, encodeURIComponent(value).replace(/%7B%7B/g, '{{').replace(/%7D%7D/g, '}}'));
        else if (where === 'query') query.push({ name, value });
        else if (where === 'header') requestHeaders.push({ name, value });
        else if (where === 'cookie') cookie.push(`${name}=${value}`);
        if (Object.keys(object(parameter.content)).length) warn(warnings, '复杂参数 content 编码未还原，请检查请求。');
      }
      if (/(?<!\{)\{([^{}]+)\}(?!\})/.test(url)) {
        warn(warnings, '路径包含未提供示例值的占位参数，请发送前填写。');
        url = url.replace(/(?<!\{)\{([^{}]+)\}(?!\})/g, '{{$1}}');
      }
      if (cookie.length) putHeader(requestHeaders, 'Cookie', cookie.join('; '));
      url = appendQuery(url, query);
      let body = '';
      const requestBody = resolve(operation.requestBody);
      {
        const content = object(requestBody.content);
        const mediaTypes = Object.keys(content);
        if (mediaTypes.length > 1) warn(warnings, '请求体有多个媒体类型，只导入一种，请核对 Content-Type。');
        const mediaType = mediaTypes.find(key => key === 'application/json') ?? mediaTypes.find(key => key === 'application/x-www-form-urlencoded') ?? mediaTypes[0];
        if (mediaType) {
          const media = object(content[mediaType]);
          const exampleEntries = Object.values(object(media.examples));
          const schema = resolve(media.schema);
          const example = media.example ?? object(exampleEntries[0]).value ?? schema.example ?? schema.default;
          if (/^multipart\/|^application\/octet-stream/i.test(mediaType)) {
            warn(warnings, `请求体 ${mediaType} 涉及文件或二进制数据，未自动生成正文。`);
          } else if (example !== undefined) {
            if (mediaType === 'application/x-www-form-urlencoded' && example && typeof example === 'object' && !Array.isArray(example)) {
              body = Object.entries(example as ObjectValue).map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(String(value))}`).join('&');
            } else if (typeof example === 'string') body = example;
            else if (/json/i.test(mediaType)) body = JSON.stringify(example, null, 2);
            else warn(warnings, `请求体 ${mediaType} 的结构化示例无法直接编码，未生成正文。`);
          } else warn(warnings, `请求体 ${mediaType} 没有示例值，未生成正文。`);
          if (body) {
            putHeader(requestHeaders, 'Content-Type', mediaType);
          }
        }
      }
      if (operation.security || root.security) warn(warnings, 'OpenAPI securitySchemes 未自动转为认证信息，请检查认证配置。');
      if (operation.callbacks || operation.webhooks) warn(warnings, '回调和 Webhook 定义未导入。');
      requests.push({ name: named(string(operation.summary) || string(operation.operationId), method, path), draft: draft(method, url, requestHeaders, body) });
    }
  }
  if (!requests.length) warn(warnings, '未找到可导入的 HTTP 操作。');
  return { source: 'openapi', collections: [{ name: title, requests }], warnings };
}

/** Legacy Insomnia v4 JSON. Current Insomnia v5 exports YAML and needs a separate parser. */
export function parseInsomnia(text: string): ExternalImportPlan {
  if (/^\s*type:\s*(?:collection|spec)\.insomnia\.rest\/5(?:\.0)?\b/m.test(text))
    throw new Error('Insomnia v5 YAML 尚未支持；请导出旧版 v4 JSON 或 HAR。');
  const root = json(text, 'Insomnia');
  if (!Array.isArray(root.resources) || (root.__export_format !== undefined && root.__export_format !== 4) ||
      (root.__export_format === undefined && !array(root.resources).every(item => typeof object(item)._type === 'string')))
    throw new Error('仅支持 Insomnia v4 JSON（resources）；v5 YAML 尚未支持。');
  const resources = array(root.resources).map(object);
  const byId = new Map(resources.map(item => [string(item._id), item]));
  const warnings: string[] = [];
  const workspaces = resources.filter(item => item._type === 'workspace');
  const collections = (workspaces.length ? workspaces : [{ _id: '', name: 'Insomnia 导入' }]).map(workspace => ({ name: bounded(string(workspace.name)) || 'Insomnia 导入', requests: [] as ImportedRequest[] }));
  const workspaceIndex = new Map(workspaces.map((item, index) => [string(item._id), index]));
  for (const item of resources.filter(resource => resource._type === 'request')) {
    let parent = string(item.parentId); const folders: string[] = []; const seen = new Set<string>(); let index = 0;
    while (parent && !seen.has(parent)) {
      seen.add(parent); const owner = byId.get(parent); if (!owner) break;
      if (owner._type === 'request_group' || owner._type === 'folder') folders.unshift(string(owner.name));
      if (owner._type === 'workspace') { index = workspaceIndex.get(parent) ?? 0; break; }
      parent = string(owner.parentId);
    }
    const method = string(item.method) || 'GET';
    let url = string(item.url);
    const parameters = array(item.parameters).map(object);
    if (parameters.some(row => row.disabled === true)) warn(warnings, 'Insomnia 禁用的查询参数未导入。');
    const query = parameters.filter(row => row.disabled !== true && string(row.name)).map(row => ({ name: string(row.name), value: string(row.value) }));
    const originalQuery = url.split('?')[1]?.split('#')[0] ?? '';
    const existing = new URLSearchParams(originalQuery);
    url = appendQuery(url, query.filter(row => !existing.getAll(row.name).includes(row.value)));
    const requestHeaders = headers(item.headers);
    let body = '';
    const requestBody = object(item.body);
    const mimeType = string(requestBody.mimeType);
    if (typeof requestBody.text === 'string') body = requestBody.text;
    else if (/application\/x-www-form-urlencoded/i.test(mimeType)) {
      body = array(requestBody.params).map(object).filter(row => row.disabled !== true && string(row.name)).map(row => `${encodeURIComponent(string(row.name))}=${encodeURIComponent(string(row.value))}`).join('&');
    } else if (array(requestBody.params).length) warn(warnings, 'Insomnia multipart 或文件正文未还原，请手动检查。');
    if (requestBody.fileName || /multipart\/form-data/i.test(mimeType)) warn(warnings, 'Insomnia 文件或 multipart 正文可能需要手动重建。');
    putHeader(requestHeaders, 'Content-Type', mimeType);
    if (Object.keys(object(item.authentication)).length) warn(warnings, 'Insomnia 认证配置未自动转换，请手动检查。');
    if (/\{%|\{\{/.test(url + body)) warn(warnings, 'Insomnia 模板变量或插件标签可能需要手动调整。');
    const label = [...folders.filter(Boolean), named(string(item.name), method, url)].join(' / ');
    collections[index].requests.push({ name: bounded(label), draft: draft(method, url, requestHeaders, body) });
  }
  if (!collections.some(collection => collection.requests.length)) warn(warnings, '未找到 Insomnia 请求。');
  if (resources.some(item => item._type === 'environment')) warn(warnings, 'Insomnia 环境变量未导入，请手动迁移。');
  return { source: 'insomnia', collections, warnings };
}

/** HAR 1.2 request snapshots. Responses and timing are outside request collections. */
export function parseHarCollection(text: string): ExternalImportPlan {
  const root = json(text, 'HAR');
  const log = object(root.log);
  if (!Array.isArray(log.entries)) throw new Error('HAR 缺少 log.entries 请求列表。');
  const warnings: string[] = [];
  const requests: ImportedRequest[] = [];
  for (const entry of array(log.entries)) {
    const request = object(object(entry).request);
    const method = string(request.method); const url = string(request.url);
    if (!method || !url) { warn(warnings, '部分 HAR 条目缺少方法或 URL，已跳过。'); continue; }
    const requestHeaders = headers(request.headers);
    if (!requestHeaders.some(row => row.name.toLowerCase() === 'cookie')) {
      const cookies = array(request.cookies).map(object).filter(row => string(row.name)).map(row => `${string(row.name)}=${string(row.value)}`);
      if (cookies.length) requestHeaders.push({ name: 'Cookie', value: cookies.join('; ') });
    }
    const postData = object(request.postData);
    let body = typeof postData.text === 'string' ? postData.text : '';
    const bodyEncoding = string(postData.encoding ?? postData._encoding).toLowerCase();
    if (bodyEncoding && bodyEncoding !== 'base64') warn(warnings, `HAR 请求体编码 ${bodyEncoding} 未识别，请检查正文。`);
    if (!body && array(postData.params).length && /application\/x-www-form-urlencoded/i.test(string(postData.mimeType))) {
      body = array(postData.params).map(object).filter(row => string(row.name)).map(row => `${encodeURIComponent(string(row.name))}=${encodeURIComponent(string(row.value))}`).join('&');
    } else if (!body && array(postData.params).length) warn(warnings, 'HAR multipart 或文件正文未还原，请手动检查。');
    putHeader(requestHeaders, 'Content-Type', string(postData.mimeType));
    const importedDraft = draft(method, url, requestHeaders, body);
    if (bodyEncoding === 'base64' && typeof postData.text === 'string') {
      try { atob(postData.text); importedDraft.bodyBase64 = postData.text; }
      catch { warn(warnings, 'HAR 中有无效的 Base64 请求体，已保留原文本，请检查正文。'); }
    }
    requests.push({ name: named('', method, url), draft: importedDraft });
  }
  warn(warnings, '仅导入 HAR 请求；响应、时序和页面信息未保存到集合。');
  if (!requests.length) warn(warnings, 'HAR 中没有可导入的请求。');
  return { source: 'har', collections: [{ name: 'HAR 导入', requests }], warnings };
}
