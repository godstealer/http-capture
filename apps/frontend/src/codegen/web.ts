import { mergedHeaders, type CodeTarget, type ExportRequest } from './types';

// JSON string literals preserve quotes, control characters and backslashes.
// Escaping '<' also keeps copied browser snippets safe inside a script element.
const literal = (value: string) => JSON.stringify(value)
  .replace(/</g, '\\u003c').replace(/\u2028/g, '\\u2028').replace(/\u2029/g, '\\u2029');

function headerEntries(request: ExportRequest): string {
  return `[\n${mergedHeaders(request).map(header =>
    `  [${literal(header.name)}, ${literal(header.value)}]`).join(',\n')}\n]`;
}

function browserBody(request: ExportRequest): string {
  return `const body = Uint8Array.from(atob(${literal(request.bodyBase64)}), character => character.charCodeAt(0));`;
}

function nodeBody(request: ExportRequest): string {
  return `const body = Buffer.from(${literal(request.bodyBase64)}, 'base64');`;
}

const bodyGuard = `if (body.length && /^(GET|HEAD)$/i.test(method)) {
  throw new Error('This client cannot send a GET/HEAD request body. Use the Node.js Native target.');
}`;

function fetchCode(request: ExportRequest, node: boolean): string {
  return `${node ? "import { Buffer } from 'node:buffer';\n\n" : ''}const url = ${literal(request.url)};
const method = ${literal(request.method)};
const headers = Object.fromEntries(${headerEntries(request)});
${node ? nodeBody(request) : browserBody(request)}
${bodyGuard}

async function main() {
  const response = await fetch(url, {
    method,
    headers,
    body: body.length ? body : undefined,
  });
  console.log(response.status, Object.fromEntries(response.headers));
  console.log(await response.text());
}

main().catch(console.error);
`;
}

function axiosCode(request: ExportRequest, node: boolean): string {
  return `import axios from 'axios';
${node ? "import { Buffer } from 'node:buffer';\n" : ''}
const url = ${literal(request.url)};
const method = ${literal(request.method)};
const headers = new axios.AxiosHeaders(Object.fromEntries(${headerEntries(request)}));
// Prevent Axios from assigning a form content type to the original bytes.
if (!headers.has('Content-Type')) headers.set('Content-Type', false);
${node ? nodeBody(request) : browserBody(request)}
${node ? '' : `${bodyGuard}\n`}
async function main() {
  const response = await axios.request({
    url,
    method,
    headers,
    adapter: '${node ? 'http' : 'xhr'}',
    data: ${node ? 'body' : 'body.buffer'},
    transformRequest: [data => data],
    responseType: 'arraybuffer',
    validateStatus: () => true,
${node ? '    maxBodyLength: Infinity,\n    maxContentLength: Infinity,\n' : ''}  });
  console.log(response.status, response.headers);
  console.log(${node ? "Buffer.from(response.data).toString('utf8')" : 'new TextDecoder().decode(response.data)'});
}

main().catch(console.error);
`;
}

function jqueryCode(request: ExportRequest): string {
  return `import $ from 'jquery';

const url = ${literal(request.url)};
const method = ${literal(request.method)};
const headers = Object.fromEntries(${headerEntries(request)});
${browserBody(request)}
${bodyGuard}

$.ajax({
  url,
  method,
  headers,
  data: body.buffer,
  processData: false,
  contentType: false,
  // Keep the response as text; never evaluate a script response.
  dataType: 'text',
}).done((data, _status, xhr) => {
  console.log(xhr.status, xhr.getAllResponseHeaders());
  console.log(data);
}).fail((xhr, status, error) => {
  console.error(xhr.status, status, error, xhr.responseText);
});
`;
}

function xhrCode(request: ExportRequest): string {
  return `const url = ${literal(request.url)};
const method = ${literal(request.method)};
const headers = ${headerEntries(request)};
${browserBody(request)}
${bodyGuard}

const xhr = new XMLHttpRequest();
xhr.open(method, url, true);
xhr.responseType = 'arraybuffer';
for (const [name, value] of headers) xhr.setRequestHeader(name, value);
xhr.onload = () => {
  console.log(xhr.status, xhr.getAllResponseHeaders());
  console.log(new TextDecoder().decode(xhr.response));
};
xhr.onerror = () => console.error('Network or CORS error');
xhr.send(body.length ? body : null);
`;
}

function nativeCode(request: ExportRequest): string {
  const flatHeaders = request.headers.flatMap(header => [header.name, header.value]);
  return `import http from 'node:http';
import https from 'node:https';
import { Buffer } from 'node:buffer';

const url = new URL(${literal(request.url)});
const method = ${literal(request.method)};
// A raw header array preserves duplicate field lines and their spelling.
const headers = [${flatHeaders.length ? `\n${flatHeaders.map(value => `  ${literal(value)}`).join(',\n')}\n` : ''}];
${nodeBody(request)}
// Raw arrays do not receive the automatic Host header from Node.js.
if (!headers.some((name, index) => index % 2 === 0 && name.toLowerCase() === 'host')) {
  headers.unshift('Host', url.host);
}
if (!headers.some((name, index) => index % 2 === 0 && name.toLowerCase() === 'content-length')) {
  headers.push('Content-Length', String(body.length));
}

const client = url.protocol === 'https:' ? https : http;
const request = client.request(url, { method, headers }, response => {
  const chunks = [];
  response.on('data', chunk => chunks.push(chunk));
  response.on('error', console.error);
  response.on('end', () => {
    console.log(response.statusCode, response.headers);
    console.log(Buffer.concat(chunks).toString('utf8'));
  });
});
request.on('error', console.error);
request.end(body);
`;
}

export const webTargets: CodeTarget[] = [
  {
    id: 'javascript-fetch', language: 'JavaScript', library: 'Fetch', filename: 'request-fetch.js',
    parser: 'javascript', browser: true, preservesDuplicates: false,
    generate: request => fetchCode(request, false),
  },
  {
    id: 'javascript-axios', language: 'JavaScript', library: 'Axios', filename: 'request-axios.js',
    parser: 'javascript', browser: true, preservesDuplicates: false, setup: 'npm install axios',
    generate: request => axiosCode(request, false),
  },
  {
    id: 'javascript-jquery', language: 'JavaScript', library: 'jQuery', filename: 'request-jquery.js',
    parser: 'javascript', browser: true, preservesDuplicates: false, setup: 'npm install jquery',
    generate: jqueryCode,
  },
  {
    id: 'javascript-xhr', language: 'JavaScript', library: 'XMLHttpRequest', filename: 'request-xhr.js',
    parser: 'javascript', browser: true, preservesDuplicates: false,
    generate: xhrCode,
  },
  {
    id: 'node-fetch', language: 'Node.js', library: 'Fetch', filename: 'request-fetch.mjs',
    parser: 'javascript', browser: false, preservesDuplicates: false, setup: 'Node.js 22+',
    generate: request => fetchCode(request, true),
  },
  {
    id: 'node-axios', language: 'Node.js', library: 'Axios', filename: 'request-axios.mjs',
    parser: 'javascript', browser: false, preservesDuplicates: false, setup: 'npm install axios',
    generate: request => axiosCode(request, true),
  },
  {
    id: 'node-native', language: 'Node.js', library: 'Native (http / https)', filename: 'request-native.mjs',
    parser: 'javascript', browser: false, preservesDuplicates: true,
    generate: nativeCode,
  },
];
