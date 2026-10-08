# HTTP Capture

跨平台 HTTP/HTTPS 调试代理。Rust/Tokio 内核，React 界面，Tauri 桌面外壳。

状态更新：2026-10-08。开发入口：[功能状态与开发路线图](docs/roadmap.md)，协议细节见 [协议说明](docs/protocols.md)。下一阶段先推进 macOS/Linux 构建验证和桌面自动打包，再完善历史管理与性能。TUN H3 排查按用户要求暂缓。

公开源码：[godstealer/http-capture](https://github.com/godstealer/http-capture)。功能实现、自动化验证和实机验收分别记录，不以编译通过代替跨平台可用。

## 项目结构

这是一个 npm + Cargo workspace 单仓库，各模块可以独立构建：

```text
apps/
  frontend/            React + TypeScript 界面、编辑器、Vite 开发代理
  desktop/src-tauri/   Tauri 桌面外壳，通过 IPC 调用 core
crates/
  capture-core/        抓包、TLS、协议解析、发送引擎、拦截、脚本和存储
  capture-service/     本地 HTTP 控制 API，供浏览器开发模式调用
vendor/h2/             保留字段顺序所需的 h2 定制版本
scripts/               跨平台启动脚本及回归检查
```

前端通过 `apps/frontend/src/api.ts` 统一调用 Tauri IPC 或本地 HTTP API。core 不依赖前端、Tauri 或 Axum；HTTP 服务和桌面端依赖同一个 core，避免维护两套业务逻辑。详见 [架构与开发说明](docs/architecture.md)。

## 本地运行

需要 Node.js 18+、Rust stable 和平台 C/C++ 工具链（Windows 为 MSVC，macOS 为 Xcode Command Line Tools，Linux 为 build-essential）。

```sh
npm ci
npm run capture
# 在另一个终端运行
npm run dev
```

打开 http://127.0.0.1:1420 ，状态应显示 `LOCAL ENGINE`。点击“开始捕获”，默认代理监听 `127.0.0.1:8080`。

Windows 当前机器在 `.local` 安装了项目独立 Rust 工具链，`npm run capture` 会使用 `scripts/rust.ps1` 加载；不会修改系统 PATH。其他机器可直接使用系统 Cargo：

```sh
cargo run -p capture-service --no-default-features
```

控制服务仅监听 `127.0.0.1:1421`。随机令牌保存在 `.local/capture/control.token`，仅 Vite 服务端读取。控制 API 不开放 CORS，开发代理检查 Host、Origin 和自定义请求头。生产桌面通过 Tauri IPC 调用同一 Rust 内核。

## 配置测试客户端

将测试客户端的 HTTP、HTTPS 代理都设为 `127.0.0.1:8080`。应用不会自动改变系统代理，也不会自动安装 CA。

HTTP 示例（Windows 使用 `curl.exe`）：

```sh
curl --noproxy "" -x http://127.0.0.1:8080 http://example.com/
```

HTTPS 需要信任本地 CA。设置面板可导出公钥证书。开发服务的默认证书路径为 `.local/capture/certificates/capture-ca.pem`。对单次 curl 测试可以只指定该证书，无须安装到系统：

```sh
curl --noproxy "" -x http://127.0.0.1:8080 --cacert .local/capture/certificates/capture-ca.pem https://example.com/
```

浏览器需要在自己的证书信任设置中导入该 CA。CA 私钥留在本地，不能分发；删除证书时应同时清理客户端的对应信任项。上游服务的证书仍正常验证，不会跳过验证。

## 当前实现与边界

| 模块 | 已实现 | 主要边界 |
| --- | --- | --- |
| HTTP/HTTPS | HTTP 显式代理、CONNECT 解密、动态 CA、H1/H2 捕获、网卡/IP/端口选择 | 跨设备与跨平台仍需实测；不自动设置系统代理或信任 CA |
| 请求保序 | H1 原始头保存；H1/H2 请求普通字段及 H2 伪头保序 | 会修正协议要求的字段；不承诺密文、HPACK/帧字节或 H3/QPACK 顺序一致 |
| 主动发送 | Auto/native/h2/h3、多标签并行、取消、cURL 导入导出、自定义原生 TLS | Auto 协商 h2/h1，不自动发现 H3；正文仍完整缓冲，单向上限 8 MiB |
| 浏览器预设 | wreq、httpcloak 独立引擎，UA 与 TLS 版本分开选择 | wreq 需 browser-replay；httpcloak 需单独构建 Go helper；不支持任意 ClientHello 编辑 |
| 代理 | HTTP/SOCKS5 列表、认证、系统凭据库；手动请求独立选择 | H3 支持 SOCKS5 UDP，不支持普通 HTTP 上游/CONNECT-UDP；失败不回退直连 |
| 查看和编辑 | 响应格式化/折叠/预览、gzip/deflate/br 查看解压、规则拦截及脚本 | 非 WebSocket/SSE 实时查看；原始正文与解码展示区分 |
| 会话 | SQLite、工作区恢复、会话 JSON 保存/导入、HAR 导入、Ctrl/Command+A 和批量删除 | 全量读取，不分页；HAR 导出、独立全库导出及大列表性能仍待完善 |
| 界面 | 主题切换、中英文及跟随系统、可拖动分栏 | 大列表虚拟滚动、部分实际 GUI 回归待补 |
| TUN | Windows 指定应用 HTTP/HTTPS 捕获，停止及父进程退出清理有实测 | H3 动态 SNI 入口有本地测试，Windows 接管未验收且暂缓；macOS/Linux TUN 未实测 |
| 发布 | Windows EXE 曾构建，GitHub 仓库及三平台 core CI 配置已建立 | 现有 EXE 不代表最新源码；桌面安装包、辅助程序集成、签名和自动更新待完成 |

H3 手动发送经 SOCKS5 到 tls3.peet.ws 已实测成功，不能据此宣称通用 TUN H3 抓包完成。详细证据见 [组件实测](docs/tls-components-test.md) 和 [TUN 文档](docs/tun.md)。长连接流式处理、trailers、请求集合/环境分组、数据库保留策略仍待实现。

## 验证

```sh
cargo test --locked -p capture-core --no-default-features
npm run build
```

本机 Windows 项目独立工具链：

```powershell
./scripts/rust.ps1 test --locked -p capture-core --no-default-features
```

也可以运行 `npm test`，会自动选择项目工具链或系统 Cargo。代理启动后执行 `node scripts/smoke.mjs`，会创建一个临时本地 HTTP 服务并发送真实请求，检查交错重复头和 UTF-8 正文；这条真实测试流量会留在 GUI 中。

测试使用本地 HTTP/TLS 服务验证头部顺序、重复字段、二进制正文、CONNECT 解密、证书验证、CA 重启、chunked、100-continue、失败落盘与停止取消；不修改系统证书信任。


### 请求与响应拦截

顶部「拦截」打开统一队列，可分别启用发送前、响应返回后断点，作用范围为全部、抓包或手动发送。首次默认关闭；开关、作用范围及已应用规则保存到本地，重启后恢复，等待放行的请求不恢复。支持原样放行、修改后放行、请求阶段直接返回自定义响应及终止；请求头编辑复用有序字段组件，保留重复字段。

每个阶段最多等待 120 秒，最多 64 个等待项；关闭相应开关会原样放行已有等待项。超时或终止不会继续发送，请求失败会返回错误。抓包的原始请求及修改前响应保存在记录的 `originalRequest` / `originalResponse` 中。正文默认以 Base64 保留原始字节，响应可复用格式化及预览；编辑压缩响应时需用解压后的正文替换，正文修改会移除原压缩和长度头。请求阶段直接替换响应不会连接目标服务器。

拦截规则：没有规则时不暂停任何流量。同一规则内 URL 包含、精确域名、方法、响应状态码、请求头名称和值包含条件须同时满足，多条规则满足任意一条即可。域名、方法、字段名不区分大小写；URL 和字段值包含区分大小写。状态码条件仅匹配响应阶段。点击「应用规则」后生效；已有等待项不再匹配新规则时会原样放行。


### JavaScript 请求前 / 响应后脚本

手动发送页面的「脚本」标签可配置两阶段脚本，并勾选启用。抓包脚本位于「拦截 → 抓包自动脚本」，点击应用后生效，复用已应用的 URL / 域名 / 方法等匹配规则，但不依赖手动拦截开关。没有规则时抓包脚本不执行；两个阶段分别按当时的数据匹配，因此响应状态码条件只在响应后生效。

执行顺序：请求前脚本 → 请求拦截 → 发送 → 响应后脚本 → 响应拦截。直接使用手动拦截替换响应时不运行响应后脚本。已完成记录保留原始请求/响应；脚本日志显示在脚本标签及记录说明中。

API：`request.method`、`request.url`、`request.headers`、`request.bodyBase64`；`response.status`、`response.headers`、`response.bodyBase64`（响应后可修改）。头部是 `{name,value}[]` 有序数组，保留重复字段。`variables` 是字符串键值字典，支持 `console.log`、`assert(condition,message)`、`encodeText(text)`、`decodeText(base64)`、`sha256(text)`。正文保留原始内容编码，decodeText 不负责解压；替换响应正文需写入未压缩内容，内核会移除旧压缩头、重算长度。

```js
// 请求前
request.headers.push({name: "X-Time", value: String(Date.now())});
console.log("发送", request.url);
// 响应后
assert(response.status === 200, "预期成功响应");
response.bodyBase64 = encodeText(JSON.stringify({ok: true}));
response.headers = [{name: "Content-Type", value: "application/json"}];
```

脚本使用内核 QuickJS 沙箱，仅支持同步 JavaScript，不兼容 Postman `pm.*`。不暴露文件、网络、模块加载或系统命令接口。每阶段 2 秒、64 MiB 内存、64 KiB 源码与变量，最多 4 段脚本同时执行；异常、断言失败、超限时终止本次请求，不应用该阶段修改。日志最多 100 行，每行 2048 字符。

手动请求变量随响应结果更新并带入下一次发送；抓包脚本配置和变量保存到本地，重启后及后续抓包请求可使用这些变量。同一变量并发写入时后完成的写入生效；重新应用配置会阻止旧脚本覆盖新配置中的变量。脚本可访问本次请求和响应，因此仅运行自己信任的代码。


脚本编辑器支持 JavaScript 高亮、行号、代码折叠、括号配对、撤销/重做和自动补全；`Ctrl+Space` 或「代码提示」可主动调用补全，`Shift+Alt+F` 或「格式化」使用 Prettier 整理代码。格式化失败会显示语法错误位置并保留原文；编辑过程中返回的旧格式化结果不会覆盖新代码。示例使用高亮代码块，可复制或追加到对应阶段。

签名辅助函数均接收 UTF-8 字符串并返回小写十六进制：`sha256(text)`、`md5(text)`、`hmacSha256(key, text)`、`hmacMd5(key, text)`。HMAC 参数顺序是密钥、消息；如接口要求大写可调用 `.toUpperCase()`。它们属于摘要或消息认证运算，不提供可逆加密。

```js
const timestamp = String(Math.floor(Date.now() / 1000));
const signature = hmacSha256(variables.secret, timestamp + request.method);
request.headers.push({ name: "X-Timestamp", value: timestamp });
request.headers.push({ name: "X-Sign", value: signature });
```

时间戳单位、待签名字符串拼接顺序和密钥使用方式应依目标 API 规范确定。


### 内置脚本环境

`encoding`、`crypto`、`utils` 随应用打包，默认启用，可在脚本编辑器分别关闭；两阶段按同一配置各自创建沙箱。旧版 `sha256` / `md5` / `encodeText` 等全局函数继续兼容，不受模块开关影响。关闭模块会移除该命名空间及其补全，底层调用也校验开关。

- `encoding.base64Encode/Decode`、`base64urlEncode/Decode`、`hexEncode/Decode` 默认与 UTF-8 转换；第二参数指定源/目标编码。
- `encoding.convert(input, {from, to})` 支持 `utf8`、`hex`、`base64`、`base64url`，支持二进制字节转换。Base64URL 输出不带填充，解码兼容有/无填充。
- `crypto.hash(algorithm, input, {inputEncoding, output})` 支持 MD5、SHA-1、SHA-256、SHA-512，默认 utf8 输入、hex 输出。
- `crypto.hmac(algorithm, {key, message, keyEncoding, inputEncoding, output})` 使用同一算法集合；keyEncoding / inputEncoding 默认 utf8，output 默认 hex。
- `crypto.randomBytes(length, output)` 使用系统安全随机数，长度 1–65536，输出默认 hex，也可 base64 / base64url。
- `utils.timestamp('seconds' | 'milliseconds')` 返回数值，默认秒；`utils.uuid()` 返回 UUID v4。

AES 当前支持 `AES-GCM`，密钥为 16 或 32 字节，nonce 为 12 字节。`crypto.encrypt/decrypt` 必须明确给出 mode、key、keyEncoding、nonce、nonceEncoding、data、inputEncoding、output；可选 aad、aadEncoding（默认 utf8）。加密输出为密文后拼接 16 字节认证标签，解密输入同格式。同一密钥下每次加密使用新 nonce，双方需保持 key / nonce / AAD 一致；认证失败不会返回明文。不支持的模式明确报错。

```js
const key = crypto.randomBytes(32, 'hex');
const nonce = crypto.randomBytes(12, 'hex');
const params = { mode: 'AES-GCM', key, keyEncoding: 'hex', nonce, nonceEncoding: 'hex' };
const encrypted = crypto.encrypt({ ...params, data: 'hello', inputEncoding: 'utf8', output: 'base64' });
const plain = crypto.decrypt({ ...params, data: encrypted, inputEncoding: 'base64', output: 'utf8' });
assert(plain === 'hello');
```

变量区域也使用 CodeMirror JSON 编辑器，支持高亮、折叠、补全和格式化，实时报告语法位置、重复名称和非字符串值。变量修改须点击「应用变量」，错误输入不会覆盖已应用变量。


### 上游代理列表

「代理设置 → 上游代理列表」支持按业务命名并新增、编辑、删除 HTTP / SOCKS5 代理，通过下拉框或「使用」切换；顶部显示当前业务名称。切换从下一条捕获请求生效，已开始的请求继续使用原配置。编辑当前代理并保存会更新后续请求的配置；删除当前代理前需先切换或选择直连。

列表名称、地址、认证用户名保存到数据目录的 `upstream-profiles.json`，密码不写入文件或返回列表接口。勾选「记住密码（系统凭据库）」后，密码保存到 Windows Credential Manager、macOS Keychain 或 Linux Secret Service，重启后自动读取；未勾选时密码仅在本次运行保留，重启后认证代理标记为「需补填密码」，默认直连，不会自动选择代理。编辑同一代理且地址、用户名未变时可留空保留当前密码；地址或用户名变化必须重新输入密码。顶部当前代理用于抓包转发；手动发送可在请求的「设置 → 本请求代理」选择直连或保存的代理。每个请求独立选择，默认直连，选择随草稿保存。Auto、native、h2、wreq、httpcloak 支持 HTTP / SOCKS5；h3 支持 SOCKS5 UDP，普通 HTTP 上游明确拒绝。不可用或失败的路径不回退直连。

取消「记住密码」或删除代理时会删除对应系统凭据。系统凭据库不可用或锁定时会显示错误，不回退到明文文件；解锁后重新打开代理设置可重试读取。Linux 桌面需提供 Secret Service（例如 GNOME Keyring / KWallet）。


### 工作区自动保存

实际创建的请求标签会自动保存到内核数据目录的 `sessions.db`，包含 URL、方法、有序请求头、正文、TLS/发送引擎配置、脚本及已应用变量。浏览器刷新或桌面重启后恢复标签，但默认仍进入抓包页，不会发送请求或自动创建空标签。关闭全部标签后保存空工作区。

界面显示读取、未保存、保存中、已保存及错误状态，编辑停止 400 ms 后保存；未保存时离开浏览器会提示。读档失败不会以空白工作区覆盖原数据，保存失败可重试。多个窗口基于版本号检测冲突，不静默覆盖另一窗口的修改。仅恢复请求草稿和结果引用，历史记录全量读取，不再限制最近 200 条。

拦截配置在应用时落盘，正则在重启时重新校验并编译；抓包脚本运行成功产生的变量增删也会保存。脚本编辑器中尚未点击“应用变量”的文字不视为已应用变量。请求正文、头部和脚本变量与抓包记录一样保存在本地数据库，系统凭据库只用于上游代理的“记住密码”。

### TUN 指定应用抓包（实验）

已支持 Windows 指定应用 HTTP/HTTPS 抓包，并实测正常停止及父进程异常退出清理。需要构建独立 Go 辅助进程，并以管理员/root 权限运行内核；使用和跨平台边界见 [TUN 文档](docs/tun.md)。旧桌面 EXE 不含此功能。

### 独立浏览器发送引擎

新增 httpcloak Go 辅助进程，支持 Chrome 143–152，UA 版本与 TLS 预设可分别设置。使用 `scripts/build-httpcloak.ps1` 构建后重启开发服务；wreq、httpcloak 和原生引擎并存。模块拆分、验证结果、跨平台构建和限制见 [transport-engines.md](docs/transport-engines.md)。
