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

export function importCurl(source: string): RequestDraft {
  let command = withoutComments(source);
  let pipedBody: string | undefined;
  // Only accept our literal byte producer, never a general shell pipeline.
  const pipe = /^printf\s+'%b'\s+'((?:\\[0-3][0-7]{2})*)'\s*\|\s*(?=curl\s)/.exec(command);
  if (pipe) { pipedBody = decodeOctal(pipe[1]); command = command.slice(pipe[0].length); }
  const args = tokenize(command);
  if (!/^curl(?:\.exe)?$/i.test(args.shift() ?? '')) throw new Error('请粘贴以 curl 或 curl.exe 开头的命令。');
  const request = emptyRequest(); request.headers = [];
  const bodies: string[] = []; let explicitMethod = false; let head = false; let jsonBody = false; let usedPipe = false;
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
    else if (['-d', '--data', '--data-raw', '--data-binary', '--json'].includes(option)) {
      const body = value();
      if (body === '@-' && option === '--data-binary' && pipedBody !== undefined && !usedPipe && !bodies.length) { usedPipe = true; continue; }
      if (usedPipe) throw new Error('不支持混合管道正文和其他正文参数。');
      if (body.startsWith('@') && option !== '--data-raw') throw new Error('不支持读取本地文件，请使用内联请求体。');
      bodies.push(body); jsonBody ||= option === '--json';
    }
    else if (option === '--url' || !option.startsWith('-')) {
      if (request.url) throw new Error('一次只能导入一个 URL。');
      request.url = option === '--url' ? value() : option;
    }
    else if (['-I', '--head'].includes(option)) head = true;
    else if (['--globoff', '--path-as-is', '--compressed', '-s', '--silent', '-S', '--show-error', '-v', '--verbose'].includes(option)) { /* No request fields to import. */ }
    else throw new Error(`暂不支持参数 ${option}，请移除或在请求编辑器中配置。`);
  }
  try { if (!['http:', 'https:'].includes(new URL(request.url).protocol)) throw new Error(); }
  catch { throw new Error('需要有效的 HTTP 或 HTTPS URL。'); }
  if (pipedBody !== undefined && !usedPipe) throw new Error('管道正文需要 --data-binary @-。');
  if (!explicitMethod) request.method = head ? 'HEAD' : bodies.length || usedPipe ? 'POST' : 'GET';
  const has = (name: string) => request.headers.some(h => h.name.toLowerCase() === name);
  if (bodies.length && !has('content-type')) header(`Content-Type: ${jsonBody ? 'application/json' : 'application/x-www-form-urlencoded'}`);
  if (!has('accept')) header(`Accept: ${jsonBody ? 'application/json' : '*/*'}`);
  request.bodyBase64 = usedPipe ? btoa(pipedBody!) : encodeText(bodies.join(jsonBody ? '' : '&'));
  return request;
}
