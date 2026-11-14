import { encodeText, type RequestDraft } from './types';

export const emptyRequest = (): RequestDraft => ({ engine: 'auto', method: 'GET', url: '', headers: [{ name: 'Accept', value: '*/*' }], bodyBase64: '', tls: { preset: 'native' } });

const withoutComments = (source: string) => source.trim().replace(/^(?:#[^\r\n]*(?:\r?\n|$)\s*)+/, '');
export const looksLikeCurl = (source: string) => /^(?:curl(?:\.exe)?(?:\s|$)|printf\s+'%b'\s+)/i.test(withoutComments(source));
const decodeOctal = (value: string) => value.replace(/\\([0-3][0-7]{2})/g, (_, digits: string) => String.fromCharCode(parseInt(digits, 8)));

// Tokenize only: clipboard content is never executed as a shell command.
function tokenize(source: string): string[] {
  const text = source.replace(/(?:\\|\^|`)\r?\n/g, '');
  const tokens: string[] = []; let token = ''; let quote = ''; let started = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (!quote && c === '$' && text[i + 1] === "'") {
      const end = text.indexOf("'", i + 2);
      const encoded = text.slice(i + 2, end);
      if (end < 0 || !/^(?:\\[0-3][0-7]{2})*$/.test(encoded)) throw new Error('暂仅支持八进制字节形式的 ANSI-C 引号。');
      token += decodeOctal(encoded); started = true; i = end; continue;
    }
    if (quote) {
      if (c === quote) { quote = ''; continue; }
      if (c === '\\' && quote === '"' && /[\\"$`]/.test(text[i + 1] ?? '')) { token += text[++i]; continue; }
      token += c; continue;
    }
    if (c === "'" || c === '"') { quote = c; started = true; continue; }
    if (/\s/.test(c)) { if (started) tokens.push(token); token = ''; started = false; continue; }
    if (/[|;&<>`]/.test(c) || (c === '$' && /[('"{]/.test(text[i + 1] ?? ''))) throw new Error('不支持 Shell 表达式，请粘贴单条普通 cURL 命令。');
    if (c === '\\') { if (++i >= text.length) throw new Error('命令末尾的转义符不完整。'); token += text[i]; }
    else token += c;
    started = true;
  }
  if (quote) throw new Error('引号未闭合。');
  if (started) tokens.push(token);
  return tokens;
}

// curl encodes only the value in name=value; field names are already encoded.
function urlencodeData(value: string): string {
  const equals = value.indexOf('=');
  if (equals < 0 && value.includes('@')) throw new Error('不支持 --data-urlencode 读取文件，请使用 name=value 内联字段。');
  const content = equals < 0 ? value : value.slice(equals + 1);
  const encoded = Array.from(new TextEncoder().encode(content), byte =>
    byte === 32 ? '+' : /[A-Za-z0-9._~-]/.test(String.fromCharCode(byte)) ? String.fromCharCode(byte) : '%' + byte.toString(16).toUpperCase().padStart(2, '0')).join('');
  return equals > 0 ? value.slice(0, equals) + '=' + encoded : encoded;
}

export function importCurl(source: string): RequestDraft {
  let command = withoutComments(source);
  let pipedBody: string | undefined;
  // Only accept our literal byte producer, never a general shell pipeline.
  const pipe = /^printf\s+'%b'\s+'((?:\\[0-3][0-7]{2})*)'\s*\|\s*(?=curl\s)/.exec(command);
  if (pipe) { pipedBody = decodeOctal(pipe[1]); command = command.slice(pipe[0].length); }
  const args = tokenize(command);
  if (!/^curl(?:\.exe)?$/i.test(args.shift() ?? '')) throw new Error('请粘贴以 curl 或 curl.exe 开头的命令。');
  const request = emptyRequest(); request.headers = [];
  const bodies: { text: string; json: boolean }[] = []; let explicitMethod = false; let head = false; let get = false; let jsonBody = false; let usedPipe = false;
  let httpVersion: '--http1.1' | '--http2' | '--http2-prior-knowledge' | '--http3-only' | undefined;
  function header(value: string) {
    const colon = value.indexOf(':');
    if (colon < 0 && value.endsWith(';')) { request.headers.push({ name: value.slice(0, -1), value: '' }); return; }
    if (colon <= 0) throw new Error('请求头必须采用 Name: Value 格式。');
    request.headers.push({ name: value.slice(0, colon), value: value.slice(colon + 1).trimStart() });
  }
  for (let i = 0; i < args.length; i++) {
    let option = args[i]; let inline: string | undefined;
    if (option.startsWith('--') && option.includes('=')) { const at = option.indexOf('='); inline = option.slice(at + 1); option = option.slice(0, at); }
    else if (/^-[XHdbAe].+/.test(option)) { inline = option.slice(2); option = option.slice(0, 2); }
    const value = () => { const result = inline ?? args[++i]; if (result === undefined) throw new Error(`${option} 缺少参数。`); return result; };
    if (['-X', '--request'].includes(option)) { request.method = value().toUpperCase(); explicitMethod = true; }
    else if (['-H', '--header'].includes(option)) header(value());
    else if (['-A', '--user-agent'].includes(option)) header(`User-Agent: ${value()}`);
    else if (['-e', '--referer'].includes(option)) header(`Referer: ${value()}`);
    else if (['-b', '--cookie'].includes(option)) { const cookie = value(); if (!cookie.includes('=')) throw new Error('不支持读取 Cookie 文件。'); header(`Cookie: ${cookie}`); }
    else if (['-d', '--data', '--data-ascii', '--data-raw', '--data-binary', '--data-urlencode', '--json'].includes(option)) {
      const body = value();
      if (body === '@-' && option === '--data-binary' && pipedBody !== undefined && !usedPipe && !bodies.length) { usedPipe = true; continue; }
      if (usedPipe) throw new Error('不支持混合管道正文和其他正文参数。');
      if (body.startsWith('@') && !['--data-raw', '--data-urlencode'].includes(option)) throw new Error('不支持读取本地文件，请使用内联请求体。');
      bodies.push({ text: option === '--data-urlencode' ? urlencodeData(body) : body, json: option === '--json' });
      jsonBody ||= option === '--json';
    }
    else if (option === '--url' || !option.startsWith('-')) {
      if (request.url) throw new Error('一次只能导入一个 URL。');
      request.url = option === '--url' ? value() : option;
    }
    else if (['-I', '--head'].includes(option)) head = true;
    else if (['-G', '--get'].includes(option)) { if (inline !== undefined) throw new Error(`${option} 不接受参数值。`); get = true; }
    else if (['--http1.1', '--http2', '--http2-prior-knowledge', '--http3-only'].includes(option)) {
      if (inline !== undefined) throw new Error(`${option} 不接受参数值。`);
      if (httpVersion && httpVersion !== option) throw new Error('一条 cURL 命令只能指定一种 HTTP 版本。');
      httpVersion = option as typeof httpVersion;
    }
    else if (['--globoff', '--path-as-is', '--compressed', '-s', '--silent', '-S', '--show-error', '-v', '--verbose'].includes(option)) { /* No request fields to import. */ }
    else throw new Error(`暂不支持参数 ${option}，请移除或在请求编辑器中配置。`);
  }
  let protocol: string;
  try { protocol = new URL(request.url).protocol; if (!['http:', 'https:'].includes(protocol)) throw new Error(); }
  catch { throw new Error('需要有效的 HTTP 或 HTTPS URL。'); }
  if (httpVersion === '--http2' && protocol === 'http:') throw new Error('明文 HTTP 的 --http2 使用 h2c Upgrade，当前发送引擎暂不支持；若服务器支持直接 h2c，可使用 --http2-prior-knowledge。');
  if (httpVersion === '--http3-only' && protocol !== 'https:') throw new Error('--http3-only 需要 HTTPS URL。');
  if (httpVersion === '--http1.1') request.engine = 'native';
  else if (httpVersion === '--http2-prior-knowledge') request.engine = 'h2';
  else if (httpVersion === '--http3-only') request.engine = 'h3';
  if (pipedBody !== undefined && !usedPipe) throw new Error('管道正文需要 --data-binary @-。');
  if (get && usedPipe) throw new Error('不支持混合 --get 和管道正文。');
  if (!explicitMethod) request.method = head ? 'HEAD' : !get && (bodies.length || usedPipe) ? 'POST' : 'GET';
  const has = (name: string) => request.headers.some(h => h.name.toLowerCase() === name);
  if (!get && bodies.length && !has('content-type')) header(`Content-Type: ${jsonBody ? 'application/json' : 'application/x-www-form-urlencoded'}`);
  if (!has('accept')) header(`Accept: ${jsonBody ? 'application/json' : '*/*'}`);
  // curl adds a separator for the current non-JSON option only when data
  // has already accumulated. JSON affects headers, not earlier separators.
  const body = bodies.reduce((result, part) => result + (result && !part.json ? '&' : '') + part.text, '');
  if (get && body) {
    const hash = request.url.indexOf('#');
    const fragment = hash < 0 ? '' : request.url.slice(hash);
    const url = hash < 0 ? request.url : request.url.slice(0, hash);
    request.url = url + (url.includes('?') ? (url.endsWith('?') ? '' : '&') : '?') + body + fragment;
  }
  request.bodyBase64 = get ? '' : usedPipe ? btoa(pipedBody!) : encodeText(body);
  return request;
}
