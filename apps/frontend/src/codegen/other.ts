import { mergedHeaders, type CodeTarget, type ExportRequest } from './types';

// Emit ordinary literals, never interpolation/raw-string syntax containing user data.
function quoted(value: string, interpolation = false, kotlin = false): string {
  let result = '"';
  for (let i = 0; i < value.length; i++) {
    const char = value[i], code = value.charCodeAt(i);
    if (char === '\\') result += '\\\\';
    else if (char === '"') result += '\\"';
    else if (char === '$' && interpolation) result += '\\$';
    else if (char === '\n') result += '\\n';
    else if (char === '\r') result += '\\r';
    else if (char === '\t') result += '\\t';
    else if (char === '\b') result += '\\b';
    else if (char === '\f' && !kotlin) result += '\\f';
    else if (code < 32 || code === 0x2028 || code === 0x2029 || code >= 0xd800 && code <= 0xdfff) result += '\\u' + code.toString(16).padStart(4, '0');
    else result += char;
  }
  return result + '"';
}
function php(value: string): string { return "'" + value.replace(/\\/g, '\\\\').replace(/'/g, "\\'") + "'"; }
function c(value: string): string {
  return '"' + [...new TextEncoder().encode(value)].map(byte => byte === 34 ? '\\"' : byte === 92 ? '\\\\' : byte === 63 ? '\\?' : byte >= 32 && byte <= 126 ? String.fromCharCode(byte) : '\\' + byte.toString(8).padStart(3, '0')).join('') + '"';
}
function chunks(value: string): string[] {
  const parts: string[] = [];
  for (let offset = 0; offset < value.length; offset += 16384) parts.push(value.slice(offset, offset + 16384));
  return parts.length ? parts : [''];
}
// JVM constants use modified UTF-8: at most three bytes per UTF-16 code unit.
// 16 KiB units therefore stay below CONSTANT_Utf8's 65535-byte limit, including
// non-ASCII text and surrogate pairs. Runtime appends prevent constant folding.
function javaString(value: string): string {
  if (value.length <= 16384) return quoted(value);
  return 'new StringBuilder()' + chunks(value).map(part => `.append(${quoted(part)})`).join('') + '.toString()';
}
function kotlinString(value: string): string {
  const q = (part: string) => quoted(part, true, true);
  if (value.length <= 16384) return q(value);
  return 'buildString { ' + chunks(value).map(part => `append(${q(part)})`).join('; ') + ' }';
}
function javaBody(request: ExportRequest, indent = '    '): string {
  return `${indent}StringBuilder encodedBody = new StringBuilder();\n${chunks(request.bodyBase64).map(part => `${indent}encodedBody.append(${quoted(part)});`).join('\n')}\n${indent}byte[] body = Base64.getDecoder().decode(encodedBody.toString());`;
}
function kotlinBody(request: ExportRequest): string {
  return `    val encodedBody = StringBuilder()\n${chunks(request.bodyBase64).map(part => `    encodedBody.append(${quoted(part)})`).join('\n')}\n    val body = Base64.getDecoder().decode(encodedBody.toString())`;
}
function curlHeader(name: string, value: string): string { return value === '' ? name + ';' : name + ': ' + value; }
function bodyExpected(request: ExportRequest): boolean { return request.bodyBytes.length > 0 || ['POST', 'PUT', 'PATCH'].includes(request.method); }
function rejectHeadBody(request: ExportRequest) {
  if (request.method === 'HEAD' && request.bodyBytes.length) throw new Error('This cURL example cannot represent a HEAD request with a body. Choose C# HttpClient or Java HttpClient.');
}
function rejectOkHttpBody(request: ExportRequest) {
  if (['GET', 'HEAD'].includes(request.method) && request.bodyBytes.length) throw new Error('OkHttp does not allow a request body for GET or HEAD. Choose Java HttpClient.');
}

// libcurl has no public Base64 decoder. Decode fixed canonical chunks locally,
// avoiding both a text round-trip and the C compiler's large-string limits.
function libcurl(request: ExportRequest): string {
  rejectHeadBody(request);
  const headers = request.headers.map(h => curlHeader(h.name, h.value));
  if (bodyExpected(request) && !request.headers.some(h => h.name.toLowerCase() === 'content-type')) headers.push('Content-Type:');
  return `#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int add_header(struct curl_slist **list, const char *value) {
    struct curl_slist *next = curl_slist_append(*list, value);
    if (!next) return 0;
    *list = next;
    return 1;
}

static size_t decode_part(const char *input, unsigned char *output) {
    const char *alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    size_t written = 0;
    for (size_t i = 0; input[i]; i += 4) {
        unsigned int value = 0;
        for (size_t j = 0; j < 4; j++) {
            const char *digit = input[i + j] == '=' ? NULL : strchr(alphabet, input[i + j]);
            value = (value << 6) | (digit ? (unsigned int)(digit - alphabet) : 0);
        }
        output[written++] = (unsigned char)(value >> 16);
        if (input[i + 2] != '=') output[written++] = (unsigned char)(value >> 8);
        if (input[i + 3] != '=') output[written++] = (unsigned char)value;
    }
    return written;
}

int main(void) {
    static const char *body_parts[] = {
${chunks(request.bodyBase64).map(part => `        ${c(part)}`).join(',\n')}
    };
    const size_t body_size = ${request.bodyBytes.length};
    unsigned char *body = NULL;
    CURL *handle = NULL;
    struct curl_slist *headers = NULL;
    CURLcode result = curl_global_init(CURL_GLOBAL_DEFAULT);
    if (result != CURLE_OK) return 1;
    body = malloc(body_size ? body_size : 1);
    handle = curl_easy_init();
    if (!body || !handle) { result = CURLE_OUT_OF_MEMORY; goto cleanup; }
    size_t offset = 0;
    for (size_t i = 0; i < sizeof(body_parts) / sizeof(body_parts[0]); i++) {
        offset += decode_part(body_parts[i], body + offset);
    }
${headers.map(header => `    if (!add_header(&headers, ${c(header)})) { result = CURLE_OUT_OF_MEMORY; goto cleanup; }`).join('\n')}
    curl_easy_setopt(handle, CURLOPT_URL, ${c(request.url)});
    curl_easy_setopt(handle, CURLOPT_HTTPHEADER, headers);
    curl_easy_setopt(handle, CURLOPT_FOLLOWLOCATION, 0L);
    curl_easy_setopt(handle, CURLOPT_TIMEOUT, 30L);
${bodyExpected(request) ? '    curl_easy_setopt(handle, CURLOPT_POSTFIELDS, (const char *)body);\n    curl_easy_setopt(handle, CURLOPT_POSTFIELDSIZE_LARGE, (curl_off_t)body_size);\n' : ''}${request.method === 'HEAD' ? '    curl_easy_setopt(handle, CURLOPT_NOBODY, 1L);\n' : ''}    curl_easy_setopt(handle, CURLOPT_CUSTOMREQUEST, ${c(request.method)});
    result = curl_easy_perform(handle); /* Response bytes are written to stdout. */
cleanup:
    if (result != CURLE_OK) fprintf(stderr, "%s\\n", curl_easy_strerror(result));
    curl_slist_free_all(headers);
    if (handle) curl_easy_cleanup(handle);
    free(body);
    curl_global_cleanup();
    return result == CURLE_OK ? 0 : 1;
}
`;
}

function dotnet(request: ExportRequest): string {
  return `using System;
using System.Net.Http;
using System.Threading.Tasks;

using var handler = new HttpClientHandler { AllowAutoRedirect = false, UseCookies = false };
using var client = new HttpClient(handler) { Timeout = TimeSpan.FromSeconds(30) };
byte[] body = Convert.FromBase64String(${quoted(request.bodyBase64)});
using var request = new HttpRequestMessage(new HttpMethod(${quoted(request.method)}), ${quoted(request.url)});
request.Content = new ByteArrayContent(body);
${mergedHeaders(request).map(h => `if (!request.Headers.TryAddWithoutValidation(${quoted(h.name)}, ${quoted(h.value)}) &&
    !request.Content.Headers.TryAddWithoutValidation(${quoted(h.name)}, ${quoted(h.value)}))
    throw new InvalidOperationException(${quoted('The HTTP client cannot set header: ' + h.name)});`).join('\n')}
using var response = await client.SendAsync(request);
Console.WriteLine((int)response.StatusCode);
await using var output = Console.OpenStandardOutput();
await response.Content.CopyToAsync(output);
`;
}

// RestSharp 113's binary BodyParameter becomes ByteArrayContent; AddJsonBody or
// legacy AddParameter(contentType, value, RequestBody) would alter the payload.
// https://github.com/restsharp/RestSharp/blob/113.0.0/src/RestSharp/Request/RequestContent.cs
function restsharp(request: ExportRequest): string {
  const headers = mergedHeaders(request);
  const agent = headers.find(h => h.name.toLowerCase() === 'user-agent')?.value;
  return `using System;
using RestSharp;

var options = new RestClientOptions {
    FollowRedirects = false,
    UserAgent = ${agent === undefined ? 'null' : quoted(agent)}
};
using var client = new RestClient(options);
client.AcceptedContentTypes = Array.Empty<string>();
if (!Enum.TryParse<Method>(${quoted(request.method)}, true, out var method) ||
    !string.Equals(method.ToString(), ${quoted(request.method)}, StringComparison.OrdinalIgnoreCase))
    throw new NotSupportedException("This HTTP method is not supported by RestSharp; choose HttpClient.");
var request = new RestRequest { Resource = ${quoted(request.url)}, Method = method, Timeout = TimeSpan.FromSeconds(30) };
byte[] body = Convert.FromBase64String(${quoted(request.bodyBase64)});
${bodyExpected(request) || headers.some(h => h.name.toLowerCase().startsWith('content-')) ? 'request.AddParameter(new BodyParameter("", body, ContentType.Binary, DataFormat.Binary));\n' : ''}${headers.filter(h => h.name.toLowerCase() !== 'user-agent').map(h => `request.AddHeader(${quoted(h.name)}, ${quoted(h.value)});`).join('\n')}
var response = await client.ExecuteAsync(request);
if (response.ErrorException is not null) throw response.ErrorException;
Console.WriteLine((int)response.StatusCode);
if (response.RawBytes is not null) {
    await using var output = Console.OpenStandardOutput();
    await output.WriteAsync(response.RawBytes);
}
`;
}

function javaHttp(request: ExportRequest): string {
  return `import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.Base64;

public class Main {
    public static void main(String[] args) throws Exception {
${javaBody(request, '        ')}
        HttpClient client = HttpClient.newBuilder()
                .followRedirects(HttpClient.Redirect.NEVER)
                .connectTimeout(Duration.ofSeconds(30)).build();
        HttpRequest.Builder builder = HttpRequest.newBuilder(URI.create(${javaString(request.url)}))
                .timeout(Duration.ofSeconds(30))
                .method(${javaString(request.method)}, HttpRequest.BodyPublishers.ofByteArray(body));
${mergedHeaders(request).map(h => `        builder.header(${javaString(h.name)}, ${javaString(h.value)});`).join('\n')}
        HttpResponse<byte[]> response = client.send(builder.build(), HttpResponse.BodyHandlers.ofByteArray());
        System.out.println(response.statusCode());
        System.out.write(response.body());
    }
}
`;
}

// Java's byte[]-first create overload and Kotlin's ByteArray.toRequestBody are
// the nondeprecated OkHttp 4.12 APIs. A null media type preserves supplied headers.
// https://github.com/square/okhttp/blob/parent-4.12.0/okhttp/src/main/kotlin/okhttp3/RequestBody.kt
function javaOkHttp(request: ExportRequest): string {
  rejectOkHttpBody(request);
  return `import java.util.Base64;
import java.util.concurrent.TimeUnit;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.RequestBody;
import okhttp3.Response;

public class Main {
    public static void main(String[] args) throws Exception {
${javaBody(request, '        ')}
        OkHttpClient client = new OkHttpClient.Builder().followRedirects(false)
                .followSslRedirects(false).callTimeout(30, TimeUnit.SECONDS).build();
        Request.Builder builder = new Request.Builder().url(${javaString(request.url)});
${request.headers.map(h => `        builder.addHeader(${javaString(h.name)}, ${javaString(h.value)});`).join('\n')}
        RequestBody requestBody = ${bodyExpected(request) || ['PROPPATCH', 'REPORT'].includes(request.method) ? 'RequestBody.create(body, (okhttp3.MediaType) null)' : 'null'};
        Request request = builder.method(${javaString(request.method)}, requestBody).build();
        try (Response response = client.newCall(request).execute()) {
            System.out.println(response.code());
            if (response.body() != null) System.out.write(response.body().bytes());
        } finally {
            client.dispatcher().executorService().shutdown();
            client.connectionPool().evictAll();
        }
    }
}
`;
}

function kotlinOkHttp(request: ExportRequest): string {
  rejectOkHttpBody(request);
  const q = kotlinString;
  return `import java.util.Base64
import java.util.concurrent.TimeUnit
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

fun main() {
${kotlinBody(request)}
    val client = OkHttpClient.Builder().followRedirects(false)
        .followSslRedirects(false).callTimeout(30, TimeUnit.SECONDS).build()
    val builder = Request.Builder().url(${q(request.url)})
${request.headers.map(h => `    builder.addHeader(${q(h.name)}, ${q(h.value)})`).join('\n')}
    val requestBody = ${bodyExpected(request) || ['PROPPATCH', 'REPORT'].includes(request.method) ? 'body.toRequestBody(null)' : 'null'}
    val request = builder.method(${q(request.method)}, requestBody).build()
    try {
        client.newCall(request).execute().use { response ->
            println(response.code)
            response.body?.let { System.out.write(it.bytes()) }
        }
    } finally {
        client.dispatcher.executorService.shutdown()
        client.connectionPool.evictAll()
    }
}
`;
}

// Dio's Uint8List branch bypasses the text/JSON transformer and computes length.
// https://github.com/cfug/dio/blob/main/dio/lib/src/dio_mixin.dart
function dio(request: ExportRequest): string {
  const q = (value: string) => quoted(value, true);
  return `import 'dart:convert';
import 'dart:io';
import 'package:dio/dio.dart';

Future<void> main() async {
  final dio = Dio();
  dio.interceptors.removeImplyContentTypeInterceptor();
  final body = base64Decode(${q(request.bodyBase64)});
  try {
    final response = await dio.request<List<int>>(
      ${q(request.url)},
      data: body,
      options: Options(
        method: ${q(request.method)},
        headers: <String, dynamic>{
${mergedHeaders(request).map(h => `          ${q(h.name)}: ${q(h.value)},`).join('\n')}
        },
        followRedirects: false,
        responseType: ResponseType.bytes,
        validateStatus: (_) => true,
      ),
    );
    stderr.writeln(response.statusCode);
    stdout.add(response.data ?? <int>[]);
    await stdout.flush();
  } finally {
    dio.close(force: true);
  }
}
`;
}

function phpCurl(request: ExportRequest): string {
  rejectHeadBody(request);
  const headers = request.headers.map(h => curlHeader(h.name, h.value));
  if (bodyExpected(request) && !request.headers.some(h => h.name.toLowerCase() === 'content-type')) headers.push('Content-Type:');
  return `<?php
declare(strict_types=1);

$body = base64_decode(${php(request.bodyBase64)}, true);
if ($body === false) throw new RuntimeException('Invalid Base64 body');
$handle = curl_init(${php(request.url)});
if ($handle === false) throw new RuntimeException('Unable to initialize cURL');
try {
    curl_setopt_array($handle, [
        CURLOPT_RETURNTRANSFER => true,
        CURLOPT_FOLLOWLOCATION => false,
        CURLOPT_TIMEOUT => 30,
        CURLOPT_HTTPHEADER => [
${headers.map(header => `            ${php(header)},`).join('\n')}
        ],
${bodyExpected(request) ? '        CURLOPT_POSTFIELDS => $body,\n' : ''}${request.method === 'HEAD' ? '        CURLOPT_NOBODY => true,\n' : ''}        CURLOPT_CUSTOMREQUEST => ${php(request.method)},
    ]);
    $response = curl_exec($handle);
    if ($response === false) throw new RuntimeException(curl_error($handle));
    fwrite(STDERR, (string) curl_getinfo($handle, CURLINFO_RESPONSE_CODE) . PHP_EOL);
    echo $response;
} finally {
    curl_close($handle);
}
`;
}

function guzzle(request: ExportRequest): string {
  return `<?php
declare(strict_types=1);
require __DIR__ . '/vendor/autoload.php';

$client = new GuzzleHttp\\Client();
$body = base64_decode(${php(request.bodyBase64)}, true);
if ($body === false) throw new RuntimeException('Invalid Base64 body');
$response = $client->request(${php(request.method)}, ${php(request.url)}, [
    'headers' => [
${mergedHeaders(request).map(h => `        ${php(h.name)} => ${php(h.value)},`).join('\n')}
    ],
    'body' => $body,
    'allow_redirects' => false,
    'http_errors' => false,
    'timeout' => 30,
]);
fwrite(STDERR, (string) $response->getStatusCode() . PHP_EOL);
echo $response->getBody();
`;
}

export const otherTargets: CodeTarget[] = [
  { id: 'c-libcurl', language: 'C', library: 'libcurl', filename: 'request.c', preservesDuplicates: true, setup: 'C99 + libcurl 开发包；cc request.c -o request $(pkg-config --cflags --libs libcurl)。HEAD 非空正文请改用 HttpClient。', generate: libcurl },
  { id: 'csharp-httpclient', language: 'C#', library: 'HttpClient', filename: 'Program.cs', preservesDuplicates: false, setup: '.NET 8+ 控制台项目；替换 Program.cs 后 dotnet run。SDK 会分组或合并同名请求头。', generate: dotnet },
  { id: 'csharp-restsharp', language: 'C#', library: 'RestSharp', filename: 'Program.cs', preservesDuplicates: false, setup: '.NET 10+；dotnet add package RestSharp --version 113.0.0；替换 Program.cs 后 dotnet run。仅支持 RestSharp Method 枚举；无 Content-Type 时二进制正文使用 application/octet-stream。', generate: restsharp },
  { id: 'java-httpclient', language: 'Java', library: 'HttpClient', filename: 'Main.java', preservesDuplicates: false, setup: 'JDK 11+；java Main.java。部分请求头及 CONNECT 方法由 JDK 限制，同名头会分组或合并。', generate: javaHttp },
  { id: 'java-okhttp', language: 'Java', library: 'OkHttp', filename: 'Main.java', preservesDuplicates: true, setup: 'JDK 8+；Gradle/Maven 添加 com.squareup.okhttp3:okhttp:4.12.0。GET/HEAD 不支持正文。', generate: javaOkHttp },
  { id: 'kotlin-okhttp', language: 'Kotlin', library: 'OkHttp', filename: 'Main.kt', preservesDuplicates: true, setup: 'Kotlin/JVM；Gradle 添加 implementation("com.squareup.okhttp3:okhttp:4.12.0")。GET/HEAD 不支持正文。', generate: kotlinOkHttp },
  { id: 'dart-dio', language: 'Dart', library: 'Dio', filename: 'request.dart', preservesDuplicates: false, setup: 'Dart 命令行项目；dart pub add dio:^5.0.0；dart run request.dart。非浏览器示例，重复请求头合并。', generate: dio },
  { id: 'php-curl', language: 'PHP', library: 'cURL', filename: 'request.php', preservesDuplicates: true, setup: 'PHP 8+ 并启用 ext-curl；php request.php。HEAD 非空正文请改用 HttpClient。', generate: phpCurl },
  { id: 'php-guzzle', language: 'PHP', library: 'Guzzle', filename: 'request.php', preservesDuplicates: false, setup: 'PHP 8+；composer require guzzlehttp/guzzle:^7.0；php request.php。重复请求头合并。', generate: guzzle },
];
