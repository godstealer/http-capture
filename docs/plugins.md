# 本地 JavaScript 插件

插件可以在匹配的 HTTP 请求发送前、响应返回后运行 JavaScript，例如添加调试头、修改本机接口的缓存头或小型 JSON 响应。当前插件 API 为 **v1**，不兼容 Burp Java、Montoya、Jython 或 Postman `pm.*` 插件。

基础插件机制已包含在正式版 0.1.19。本页补充的说明、示例、界面帮助和在线编辑已随正式版 0.1.23 发布；正文共享和序列化优化的发布边界见 [内存改造记录](body-memory.md)。手工验收继续见 [本轮清单](acceptance-2026-10-19.md)。

## 第一次使用

1. 打开左侧「插件」，选择本地 JSON 文件，或点击「编写插件」直接编辑；也可在「使用说明」中选择一个内置示例，先从只改请求头的「本机请求标识」开始。
2. 导入或选择示例后，展开「查看插件代码」，确认匹配地址和代码，再点击「安装插件」；也可选「编辑后安装」调整。编写界面使用「检查语法」或「检查并保存插件」。只预览、编辑或检查语法不会安装。
3. 勾选「启用插件」，向已有的本机测试 API 发送普通 HTTP 请求，例如 `http://127.0.0.1:8000/echo`；查看请求头和发送说明，并在插件页点击「刷新」查看日志。

**通过界面新安装的插件始终禁用**，即使文件写了 `enabled: true`，也需要安装后启用。示例不会启动服务器、自动发送请求或设置系统代理；本机没有对应服务时，连接失败不表示插件安装失败。

启停、上移、下移、卸载均立即保存，不需要另点总保存。列表从上到下执行，可用「上移」「下移」调整顺序；筛选后的排序按钮仍调整完整列表顺序。配置、代码、顺序和启用状态保存在当前内核数据库中。

列表支持按名称、ID、版本或匹配地址搜索，以及启用状态、来源范围筛选；「详情」一次查看一个插件的代码，避免安装多个插件后页面过长。工具栏的「完整说明」位于可展开的「使用说明」内，随应用打包，不依赖网络，支持选中和复制全文。

### 在应用内编写或修改

「编写插件」提供 ID、名称、版本、范围、URL 正则及请求前/响应后两个 CodeMirror 6 JavaScript 编辑框，支持高亮与格式化。空钩子表示不运行该阶段；默认模板只匹配本机 HTTP。

「检查语法」仅校验清单、Rust 正则和两段 JavaScript 的编译语法，**不执行代码、不安装、不保存、不发送网络请求**。未知 API、运行时类型、正文编码、输出校验和资源超限仍要在实际请求中测试；语法通过不表示业务逻辑正确。同步钩子不支持 `await`、异步任务或外部模块加载。

「检查并保存插件」检查成功后才替换安装列表；出错时保留编辑内容和原插件。编辑已安装插件时 ID 只读，成功保存后该插件变为禁用，需要重新勾选启用；检查期间已安装的旧版本保持原状。新建、编辑、JSON 导入两种形式共用同一内核插件机制，不需要本地构建工具。

当前不提供插件包导出。保留自己的 JSON 文件仍有助于备份、跨实例迁移；导入同 ID 不自动覆盖，需在界面编辑原插件，或卸载旧版再导入。

## 可以直接导入的示例

所有示例显式设置 `enabled: false`，没有真实令牌，不依赖跨钩子状态。

| 文件 | 范围与匹配 | 作用 |
| --- | --- | --- |
| [local-request-id.json](../examples/plugins/local-request-id.json) | 手动 HTTP；本机 HTTP，允许端口 | 替换同名调试头，生成 `X-Debug-Request-Id`；只有请求前代码，适合入门。 |
| [example-json-response.json](../examples/plugins/example-json-response.json) | 手动 HTTP；`example.invalid/api/`，允许端口 | 给未压缩的 UTF-8 JSON 对象增加 `debugPlugin` 字段；跳过非 JSON、非 UTF-8、空正文及非对象根节点。 |
| [local-no-cache.json](../examples/plugins/local-no-cache.json) | 抓包及手动 HTTP；本机 `/demo/` 路径 | 移除条件缓存头，添加请求 `no-cache` 和响应 `Cache-Control: no-store`，不改正文。 |
| [demo-header.json](../examples/plugins/demo-header.json) | 抓包及手动 HTTP；`127.0.0.1` | 最小请求/响应头追加示例，会保留同名重复项。 |

本机示例匹配 `http://127.0.0.1/` 或 `http://localhost/`。`example.invalid` 不是业务站点，不能直接访问来验证；测试前将 JSON 中的 `urlPattern` 改为自己的本机接口，再重新安装。带响应后代码的示例需要避开 SSE 接口，详见运行限制。

## JSON 插件包

一个文件放一个对象，不是插件数组，也不包含外层 `plugins`：

```json
{
  "id": "my-local-header",
  "name": "我的本机调试头",
  "version": "1.0.0",
  "apiVersion": 1,
  "enabled": false,
  "scope": "replay",
  "urlPattern": "^http://127\\.0\\.0\\.1:8000/",
  "before": "request.headers.push({name: 'X-Debug', value: 'hello'}); console.log('header added');",
  "after": ""
}
```

| 字段 | 类型与要求 | 含义 |
| --- | --- | --- |
| `id` | 必填字符串，1～80 个 ASCII 字符；只允许字母、数字、`.`、`-`、`_` | 本地唯一标识，同 ID 不能同时安装。 |
| `name` | 必填非空字符串，UTF-8 不超过 128 字节 | 界面名称，中文按字节计限额。 |
| `version` | 必填非空字符串，UTF-8 不超过 40 字节 | 展示版本；不强制 SemVer，不触发自动升级。 |
| `apiVersion` | 必填整数，目前只能为 `1` | 插件协议版本。 |
| `enabled` | 布尔值，内核缺省 `false` | GUI 导入时强制变为 `false`；文件中建议明确写出。 |
| `scope` | 必填：`capture`、`replay`、`all` | 普通 HTTP 抓包、手动 HTTP 发送或两者，不代表所有网络协议。 |
| `urlPattern` | 必填字符串，UTF-8 不超过 2048 字节 | Rust regex 正则，匹配完整 URL；空正则匹配任意 URL。 |
| `before` | GUI 包必须提供字符串，可为 `""`；内核缺省空 | 请求前代码，最多 65536 个 UTF-8 字节。 |
| `after` | GUI 包必须提供字符串，可为 `""`；内核缺省空 | 响应后代码，最多 65536 个 UTF-8 字节。 |

界面导入文件上限为 **150000 字节**，最多安装 **32 个插件**。内核拒绝未知字段，不能自行添加 `permissions`、`modules` 等字段。直接安装 JSON 包时校验清单和正则；编写界面的检查/保存还纯编译两段 JavaScript。API 与输出仍要在钩子实际运行时校验。

JSON 不允许注释或末尾多余逗号；代码换行写成 `\n`。Rust regex 不是 glob 或 JavaScript 正则：`^`/`$` 限制开头/结尾，不加锚点时可匹配任意片段；域名句点在正则中写成 `\.`，在 JSON 中写成 `\\.`。默认区分大小写，可加 `(?i)`；不支持回溯引用或前后查找，例如 `(?=...)`。

插件匹配使用本次普通 HTTP 执行开始时的 URL、来源及列表快照。插件改 URL 后不重新计算本次匹配；修改插件配置对下一次请求生效。

## 钩子里的对象与工具

每个钩子在独立 QuickJS 上下文执行。直接写代码，不需要函数名或返回请求/响应。

| 对象或函数 | 当前行为 |
| --- | --- |
| `request.method`、`request.url` | 方法和绝对 URL；`before` 修改后经过内核请求校验。 |
| `request.headers` | 有序 `{name, value}` 数组，可追加、替换、过滤；字段名匹配时自行处理大小写。 |
| `request.bodyBase64` | 请求实体原始 Base64 文本，空正文为 `""`。 |
| `response.status`、`response.headers`、`response.bodyBase64` | `after` 可读取/修改，状态码须为 200～599。 |
| `variables` | 本钩子的字符串字典，每次从空字典开始；不读取手动请求脚本变量，不跨阶段/请求持久化。 |
| `console.log(...)` | 写入插件日志和请求发送说明，不输出系统终端。 |
| `assert(condition, message)` | 不满足时抛错并回滚本钩子，**不能用于阻止请求继续发送**。 |
| `encodeText(text)`、`decodeText(base64)` | UTF-8 与 Base64 转换；不解压 gzip/br，不将任意二进制当作文本。 |
| `sha256(text)`、`md5(text)` | UTF-8 字符串的小写十六进制摘要。 |
| `hmacSha256(key, text)`、`hmacMd5(key, text)` | UTF-8 HMAC，参数顺序为密钥、消息。 |

`request`、`response` 还有协议/脚本配置字段，请保留无须修改的字段，不要重建成缺字段对象。响应协议、TLS 等事实字段仍由内核保留。`before` 对响应的修改不提交，`after` 对请求的修改不提交。

插件默认可用三个工具模块；插件包暂不能配置模块开关，与手动请求脚本设置分别处理：

| 模块 | 函数 |
| --- | --- |
| `encoding` | `convert(input, {from, to})` 支持 `utf8`/`hex`/`base64`/`base64url`；还有 `base64Encode/Decode`、`base64urlEncode/Decode`、`hexEncode/Decode`。例如 `encoding.base64Decode(response.bodyBase64, 'hex')` 返回二进制 Hex。 |
| `crypto` | `hash(algorithm, input, options)`、`hmac(algorithm, options)` 支持 MD5/SHA-1/SHA-256/SHA-512；`randomBytes(length, output)` 支持 1～65536 字节；`encrypt/decrypt(options)` 仅支持 AES-GCM。 |
| `utils` | `uuid()`、`timestamp('seconds' \| 'milliseconds')`；没有请求开始时间或跨阶段计时对象。 |

AES-GCM 的编码、密钥和 nonce 约定见 [内置脚本环境](../README.md#内置脚本环境)。不要把真实令牌或密钥写进 JSON；本地插件没有独立凭据库或持久化变量配置。

## 常见修改

### 替换请求头

放在 `before`，先删去同名字段再追加，避免制造重复项：

```js
request.headers = request.headers.filter(
  header => header.name.toLowerCase() !== 'x-debug-mode'
);
request.headers.push({name: 'X-Debug-Mode', value: 'local'});
console.log('debug header added');
```

如果就是要测试重复字段，可以直接 `push`。不要加入换行，也不要伪造 `Host` 或 `Content-Length`：这些字段仍受 URL、正文字节数和协议约束。

### 修改 JSON 响应

先确认是未压缩 UTF-8 JSON，再用 `JSON.parse(decodeText(response.bodyBase64))` 读取，修改后用 `encodeText(JSON.stringify(data))` 写回。完整检查见 [JSON 示例](../examples/plugins/example-json-response.json)。

正文变化后，内核移除旧 `Content-Encoding`、`Content-Length`、`Transfer-Encoding` 并重算长度。插件不自动解压；不要将图片、文件或压缩字节直接交给 JSON 处理。

### 只记录状态

放在 `after`：

```js
console.log('status', response.status);
console.log('header count', response.headers.length);
```

这仍是响应后钩子，受正文大小和 SSE 限制。日志不是脱敏工具，主动输出 Cookie、Authorization、正文会将这些内容留在本机记录里。

## 执行顺序与失败

```text
Block 检查
→ 插件 before（从上到下）
→ 请求前脚本
→ 请求断点
→ 再次 Block 检查与发送
→ 插件 after（仍从上到下）
→ 响应后脚本
→ 响应断点
```

插件不依赖手动脚本开关或断点开关。抓包自动脚本另受已应用规则约束，不代替插件自己的匹配条件。

成功钩子的修改交给下一个插件。抛错、超时、繁忙、无效字段或超限时，丢弃该钩子修改并继续后续流程。需要阻止请求时使用 Block，不能依赖 `throw` 或 `assert`。

原始请求/响应另行保留。Block、取消、发送失败或请求断点直接返回自定义响应时，响应后钩子可能不执行；不能假设每个 `before` 都对应一次 `after`。

## 日志在哪里

- 插件页点击「刷新」获取当前日志，不是实时订阅。
- 日志带 Flow ID、插件 ID、`request`/`response` 阶段及成功/错误，例如 `[plugin local-request-id request] OK ...`。
- 对应请求的发送说明也保留插件记录，随请求落盘。
- 插件页保留当前内核进程最近 200 条，重启清空；单条插件执行消息最多 8192 个字符。
- 每个钩子最多记录 100 次 `console.log`，每次输出截断至 2048；合并后还受消息总长度限制，不适合完整正文导出。

## 排错

| 现象 | 检查方式 |
| --- | --- |
| 选择文件后没安装 | 先预览再点击「安装插件」；查看错误。GUI 要求 `id/name/version/scope/urlPattern/before/after` 为字符串、`apiVersion` 为整数，文件不超过 150000 字节。 |
| 装好但没日志 | 确认启用、scope、原始 URL 和 Rust 正则；空白钩子不执行，历史记录不重跑。 |
| 重复 ID/不支持 API | 卸载同 ID 旧版再装；`apiVersion` 必须为 1，更改 `version` 不触发更新。 |
| 语法通过但运行报错 | 语法检查不执行代码；检查对象字段、编码和 API 是否存在，再看实际请求的插件日志。 |
| 改动没生效 | 查看钩子错误，失败会回滚；后续插件、脚本、断点也可能覆盖修改。 |
| `decodeText`/`JSON.parse` 失败 | 检查 MIME、内容编码、字符集、空正文；压缩或二进制不是 UTF-8 JSON。 |
| 只改响应头也超限 | 响应后仍校验完整正文，当前最多 8 MiB；缩小匹配范围。 |
| SSE 请求失败 | 非空 `after` 会触发实时流保护；规则排除流接口，或将 `after` 留空。不能靠钩子内提前返回绕过。 |
| `fetch`/`require`/`pm` 不存在 | 没有网络、模块加载或 Postman 运行时，使用同步代码及内置工具。 |
| 响应阶段 variables 为空 | 每个钩子从空字典开始，不跨阶段传递。 |
| 换数据目录后找不到插件 | 确认连接的开发内核/桌面数据目录，不直接合并数据库。 |

## 运行限制

- 插件能读取匹配流量全部内容并修改目的 URL，只启用可信代码；没有按字段/域名细分的能力授权。
- 同步 JavaScript，不支持 async、Promise、文件、进程、独立网络、Node `require` 或浏览器 DOM。
- 每钩子限时 2 秒、内存 64 MiB、栈 256 KiB；与脚本共用最多 4 个执行槽，忙时记录失败并回滚该钩子。
- 结果 JSON 最多 24 MiB、变量序列化最多 64 KiB；响应实体最多 8 MiB、响应头合计最多 64 KiB。工具函数另有输入限制，不是大文件转换器。
- 普通 HTTP 抓包和手动 HTTP 发送共用钩子。WebSocket 握手/消息、主动交互式 gRPC、TUN 特殊流量尚未全面接入，不能推广至所有网络流量。
- 尚无扩展 UI 面板、右键菜单注册、扫描器、市场、自动升级、插件签名校验、异步脚本或细粒度能力授权。

## 内核验证

插件顺序、回滚、禁用、匹配和持久化已有内核集成测试。示例契约测试使用替身发送引擎与真实 QuickJS，不访问业务网站：

```sh
node scripts/cargo.mjs test --locked -p capture-core --no-default-features --test plugins --test plugin_examples
```

自动化不能替代目标平台、你自己的接口和具体插件代码的使用验收。
