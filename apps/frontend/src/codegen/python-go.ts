import { mergedHeaders, type CodeTarget, type ExportRequest } from './types';

// JSON's quoted-string escapes are also valid in Python string literals. Keep
// request bytes in Base64 rather than reserializing text, JSON, or form data.
const pythonString = (value: string): string => JSON.stringify(value);

function goString(value: string): string {
  let quoted = '"';
  for (const character of value) {
    const point = character.codePointAt(0)!;
    if (character === '"') quoted += '\\"';
    else if (character === '\\') quoted += '\\\\';
    else if (point >= 0x20 && point <= 0x7e) quoted += character;
    else if (point <= 0x7f) quoted += `\\x${point.toString(16).padStart(2, '0')}`;
    else if (point >= 0xd800 && point <= 0xdfff) quoted += '\\ufffd';
    else if (point <= 0xffff) quoted += `\\u${point.toString(16).padStart(4, '0')}`;
    else quoted += `\\U${point.toString(16).padStart(8, '0')}`;
  }
  return quoted + '"';
}

function pythonRequests(request: ExportRequest): string {
  const headers = mergedHeaders(request).map(header =>
    `    ${pythonString(header.name)}: ${pythonString(header.value)},`).join('\n');
  return `import base64
import requests

url = ${pythonString(request.url)}
headers = {
${headers}
}
body = base64.b64decode(${pythonString(request.bodyBase64)}, validate=True)

response = requests.request(
    method=${pythonString(request.method)},
    url=url,
    headers=headers,
    data=body,
    timeout=30,
    allow_redirects=False,
)
print(response.status_code)
print(response.text)
`;
}

function pythonHttpClient(request: ExportRequest): string {
  const headers = request.headers.map(header =>
    `    (${pythonString(header.name)}, ${pythonString(header.value)}),`).join('\n');
  return `import base64
import http.client
from urllib.parse import quote, urlsplit

url = ${pythonString(request.url)}
method = ${pythonString(request.method)}
headers = [
${headers}
]
body = base64.b64decode(${pythonString(request.bodyBase64)}, validate=True)

parts = urlsplit(url)
if parts.scheme not in ("http", "https") or not parts.hostname:
    raise ValueError("An absolute HTTP or HTTPS URL is required")
connection_type = http.client.HTTPSConnection if parts.scheme == "https" else http.client.HTTPConnection
connection = connection_type(parts.hostname, parts.port, timeout=30)
host = parts.hostname.encode("idna").decode("ascii")
if ":" in host:
    host = "[" + host + "]"
if parts.port is not None:
    host += ":" + str(parts.port)
target = quote(parts.path or "/", safe="/%:@!$&'()*+,;=-._~")
if parts.query:
    target += "?" + quote(parts.query, safe="%/:?@!$&'()*+,;=-._~[]")

try:
    connection.putrequest(method, target, skip_host=True, skip_accept_encoding=True)
    connection.putheader("Host", host)
    for name, value in headers:
        connection.putheader(name, value)
    if body or method.upper() in ("POST", "PUT", "PATCH"):
        connection.putheader("Content-Length", str(len(body)))
    connection.endheaders(body)
    response = connection.getresponse()
    print(response.status, response.reason)
    print(response.read())
finally:
    connection.close()
`;
}

function goNetHttp(request: ExportRequest): string {
  const headers = request.headers.map(header =>
    `\treq.Header.Add(${goString(header.name)}, ${goString(header.value)})`).join('\n');
  return `package main

import (
\t"bytes"
\t"encoding/base64"
\t"fmt"
\t"io"
\t"net/http"
\t"os"
\t"time"
)

func main() {
\tbody, err := base64.StdEncoding.DecodeString(${goString(request.bodyBase64)})
\tif err != nil {
\t\tpanic(err)
\t}
\treq, err := http.NewRequest(${goString(request.method)}, ${goString(request.url)}, bytes.NewReader(body))
\tif err != nil {
\t\tpanic(err)
\t}
${headers}

\tclient := &http.Client{
\t\tTimeout: 30 * time.Second,
\t\tCheckRedirect: func(req *http.Request, via []*http.Request) error {
\t\t\treturn http.ErrUseLastResponse
\t\t},
\t}
\tresponse, err := client.Do(req)
\tif err != nil {
\t\tpanic(err)
\t}
\tdefer response.Body.Close()
\tfmt.Println(response.Status)
\tif _, err := io.Copy(os.Stdout, response.Body); err != nil {
\t\tpanic(err)
\t}
}
`;
}

export const pythonGoTargets: CodeTarget[] = [
  {
    id: 'python-requests', language: 'Python', library: 'Requests', filename: 'request.py', parser: 'python',
    preservesDuplicates: false, setup: 'pip install requests', generate: pythonRequests,
  },
  {
    id: 'python-http-client', language: 'Python', library: 'http.client', filename: 'request.py', parser: 'python',
    preservesDuplicates: true, generate: pythonHttpClient,
  },
  {
    id: 'go-net-http', language: 'Go', library: 'net/http (Native)', filename: 'main.go', parser: 'go',
    preservesDuplicates: true, generate: goNetHttp,
  },
];
