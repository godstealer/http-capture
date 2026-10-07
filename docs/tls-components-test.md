# TLS 组件实测

时间：2026-09-29T16:07:06.711Z

目标：https://tls.peet.ws/api/all 。通过前端 /__capture/replay 调用；UA 使用用户提供的 Chrome 152。HTTP/SOCKS5 使用本机 7897 代理。httpcloak 覆盖全部公布版本；wreq 每个浏览器抽测最新版本，未逐个验证旧版本。

| 引擎 | 预设 | 路径 | 结果 | 响应 | 错误 |
|---|---|---|---|---|---|
| native | native  | HTTP | PASS | 200 HTTP/1.1 TLS 1.3 |  |
| native | native  | SOCKS5 | PASS | 200 HTTP/1.1 TLS 1.3 |  |
| auto | native  | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| h2 | native  | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| auto | native  | SOCKS5 | PASS | 200 HTTP/2 TLS 1.3 |  |
| h2 | native  | SOCKS5 | PASS | 200 HTTP/2 TLS 1.3 |  |
| h3 | native  | HTTP | UNSUPPORTED |    | h3 send engine: HTTP/3 不支持此上游代理，未回退直连 |
| httpcloak | chrome 152 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | chrome 151 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | chrome 150 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | chrome 149 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| auto | native  | direct | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | chrome 147 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | chrome 148 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | chrome 145 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | chrome 146 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | chrome 143 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | chrome 144 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | chrome 152 | SOCKS5 | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | firefox 148 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | firefox 133 | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | firefox 148 | SOCKS5 | PASS | 200 HTTP/2 TLS 1.3 |  |
| wreq | chrome 149 | HTTP | UNSUPPORTED |    | wreq send engine: 此发送引擎暂不支持上游代理；未回退直连 |
| wreq | chrome 149 | direct | FAIL |    | wreq send engine: error decoding response body: error reading a body from connection: error reading a body from connection: connection reset |
| wreq | firefox 151 | HTTP | UNSUPPORTED |    | wreq send engine: 此发送引擎暂不支持上游代理；未回退直连 |
| httpcloak | chrome auto | HTTP | PASS | 200 HTTP/2 TLS 1.3 |  |
| httpcloak | chrome 999 | HTTP | UNSUPPORTED |    | httpcloak send engine: chrome 999 TLS profile is not available in this build; choose a supported version or latest explicitly (latest: 152) |
| native | native 1.2 | HTTP | PASS | 200 HTTP/1.1 TLS 1.2 |  |
| native | native 1.3 | HTTP | PASS | 200 HTTP/1.1 TLS 1.3 |  |
| h3 | native  | direct | FAIL |    | h3 send engine: timed out |
| wreq | firefox 151 | direct | FAIL |    | wreq send engine: request or response body error: operation timed out: operation timed out |

PASS 仅表示返回 200、可解析 TLS 回显且 UA 未被改写，不代表完整浏览器指纹保真。GUI 点击、TUN、HTTP/3 代理未在此脚本中验证。

## 其他组件验证

| 组件 | 结果 | 验证方式 |
|---|---|---|
| HTTP/1.1 HTTPS 抓包解密 | 通过 | curl → 8080 → tls.peet.ws，200，双方 TLS1.3，原始请求头及可解析 JSON 已记录 |
| HTTP/2 HTTPS 抓包解密 | 通过 | Node h2 → 8080 → 已保存 HTTP 上游 → tls.peet.ws，200/h2；客户端显式提供正确的 scheme/authority |
| 请求前脚本 | 通过 | SHA256 生成测试头，目标站点的 HEADERS 回显包含该头 |
| 响应后脚本 | 通过 | 修改响应头、写入变量200、保留原始响应 |
| 拦截规则、修改替换、取消 | 通过 | 本地隔离回归测试；未更改现用拦截规则 |
| HTTP/SOCKS5 代理、认证与拒绝 | 通过 | 实际站点回显 + 本地 CONNECT/认证/407 集成测试 |
| JSON/HTML/JS/CSS 格式化、Brotli 解码 | 通过 | 前端组件测试（目标返回JSON，其他类型使用测试样本） |
| cURL 导入导出、HAR/会话 | 通过 | 前端往返及边界测试 |
| 并行请求、取消及响应路由 | 通过 | 前端请求状态测试 |
| 国际化及代理选择组件 | 通过 | 翻译覆盖与组件静态渲染测试 |
| 前端生产构建 | 通过 | TypeScript + Vite；仍有既存大包警告 |

修复：RequestProxySelector 原先硬编码只允许 auto/native/h2，导致 httpcloak 的代理选项禁用。已加入 httpcloak，并补充组件回归测试。前端热更新即可生效。

环境说明：本轮 Auto 直连曾成功，之后 H2 抓包直连发生连接重置；网络路径表现有波动，不能归结为站点永久不可直连。HTTP/2 抓包通过上游代理验证成功，测试后恢复原全局直连配置。Windows Schannel curl 首次因本地CA没有撤销信息拒绝，使用仅本次调用的 --ssl-revoke-best-effort 后完成验证，未关闭证书链/主机名校验。

未验收：GUI 真实点击及代码折叠/预览交互、TUN 接管、跨平台实机、wreq 全部旧版本、HTTP/3 代理。HTTP/3 直连超时不代表已确定原因，需单独检查目标站点 QUIC 支持与 UDP 路径。上表为首次测试记录，wreq 上游的后续修复见下文；直连失败仍未解决。Chrome999 是预期拒绝的负向用例。

## 追加验证：wreq 上游代理（2026-09-30）

wreq 已接入 HTTP CONNECT 和 SOCKS5，前端代理选择已启用。通过相同前端 API、相同目标与 Chrome152 UA 验证：

| TLS 预设 | HTTP 上游 | SOCKS5 上游 |
|---|---|---|
| Chrome149 | 200 / HTTP2 / UA 保留 | 200 / HTTP2 / UA 保留 |
| Firefox151 | 200 / HTTP2 / UA 保留 | 200 / HTTP2 / UA 保留 |

四次服务器 TLS 回显均为 772（TLS1.3）；这不是 wreq 本地协商详情字段。最新结果保存在 `.local/wreq-proxy-results.json`。矩阵脚本已增加 wreq SOCKS5 用例，未重复运行整套矩阵。

验证通过：fidelity 3 项、upstream 7 项、httpcloak/wreq 集成 3 项、multiplex 5 项，以及国际化检查和前端构建。新增 wreq 测试验证 CONNECT 认证、407 拒绝后不回退直连及流量记录不含凭据。SOCKS5 使用远端 DNS；明确关闭环境代理自动继承。

H3 增加 10 秒 QUIC 握手超时及分阶段错误。tls.peet.ws 实测仍在握手阶段超时，尚未获得 HTTP 响应；本地 H3 握手、证书校验及二进制响应测试通过，不能据此宣称外部 H3 已跑通。

补充外部探测 `https://cloudflare-quic.com/` 同样在 QUIC 握手阶段超时。两个端点的观测尚不足以区分 UDP 网络路径、端点支持和其他握手兼容问题，根因未确定。更新后的服务已运行，监听恢复为 `127.0.0.1:8080`。

## H3 多地址修复（2026-09-30）

修复原先只连接 DNS 第一个地址的问题：去重后最多并行尝试 16 个地址，首个成功连接用于请求，其余尝试取消；保留证书与主机名校验。每个地址握手上限 10 秒，全部失败时报告各地址原因。

新增黑洞 UDP 地址在前、正常 QUIC 地址在后的回归测试，通过（约 20ms）；原有 multiplex 5 项全部通过，包括可信 CA、H3 反向代理和二进制正文往返。

新增 `quic-probe` 命令，可显式访问外部 HTTPS URL 并报告 H3 结果，不回退 TCP。多地址修复后，tls.peet.ws 的 IPv4 和 cloudflare-quic.com 的两个 IPv4、两个 IPv6 地址仍全部握手超时。额外 Google 探测也超时，但其本机 DNS 结果异常，不作为可靠控制端点。外网 H3 尚未验收，不将故障归因为已确定的 UDP 封锁。

支持边界：已有原生 H3 手动发送、指定目标的回环 QUIC 反向代理；尚无任意目标的 TUN QUIC 自动接管、SOCKS5 UDP ASSOCIATE / CONNECT-UDP 出站、QPACK 线上字段顺序保真。Auto 目前仍是 h2/h1 协商。

## H3 SOCKS5 UDP 出站（2026-10-01，更新上述边界）

已实现 SOCKS5 UDP ASSOCIATE，包括无认证/用户名密码认证、代理端目标域名解析、IPv4/IPv6 封装、关联控制连接生命周期及取消清理。每个请求独立中继，代理失败不回退直连。普通 HTTP CONNECT 仍不支持 H3；界面只允许 H3 选择 SOCKS5 配置。实现依据 [RFC 1928](https://www.rfc-editor.org/rfc/rfc1928)。

QUIC 与本机中继之间只传输密文，SNI 和证书验证仍使用真实目标域名；固定 1200 字节 QUIC 数据报，避免将回环路径 MTU 错当作代理出口 MTU。当前不支持 SOCKS 分片，按规范丢弃非零 FRAG 数据报。

- 新增本地带认证 SOCKS5 UDP → H3 反向代理 → H3 服务器的 100KB 二进制往返测试，成功并验证控制连接释放。
- multiplex 全部 6 项、UDP 封装/拒绝/控制关闭 2 项、国际化与前端生产构建通过。
- 通过本机 `socks5://127.0.0.1:7897`，`https://cloudflare-quic.com/` 实测 **200 / HTTP3 / TLS1.3，约 467ms**。
- 相同代理访问 `https://tls.peet.ws/api/all` 仍在 QUIC 握手阶段超时；未确认目标 H3 支持，不能宣称该 URL 的 H3 通过。
- 待办：CONNECT-UDP、TUN 任意目标接管、QPACK 顺序保真；本次没有实现这些能力。

独立复测：先构建 `capture-service` 的 `quic-probe` 二进制，再执行 `target/debug/quic-probe.exe --proxy socks5://127.0.0.1:7897 https://cloudflare-quic.com/`。不输出正文或代理凭据。

部署后通过前端 `/__capture/replay` API 复验成功：H3 + 已保存的 SOCKS5 配置访问 cloudflare-quic.com，200 / HTTP3 / TLS1.3、证书链 3 张、无错误；结果保存在 `.local/h3-socks-api-result.json`。原监听 `127.0.0.1:8080` 已恢复。此次验证为 API 路径，未宣称 GUI 实际点击验收。

## Peet H3 回显与边界回归（2026-10-01）

[TrackMe 项目](https://github.com/pagpeter/TrackMe) 公布了 `tls.peet.ws` 和 `tls3.peet.ws` 两个域名。实测后确认可用的 H3 回归端点为 **https://tls3.peet.ws/api/all**：经 SOCKS5，前端 API 返回 200 / HTTP3 / TLS1.3，服务器 JSON 包含 `http_version: h3`、`http3`、`tls`，Chrome152 UA 保持不变。该结论来自实际响应，不是根据域名中的“3”推断。

新增复测入口：`node scripts/test-tls-components.mjs --h3`。实测 SOCKS5 通过（约 991ms、4 张证书）；直连仍超时；普通 HTTP 上游按预期拒绝。详见 [生成的 H3 矩阵](h3-matrix-generated.md)。脚本的自动报告与本人工维护记录已分开，避免重跑覆盖历史说明。

新增测试覆盖：不可信 CA、证书主机名不匹配、目标域名以 SOCKS 地址字段传递（无需本地解析）、握手取消后 TCP 关联释放。核心单元测试 18 项通过，1 项真实系统凭据测试按原设置 ignored。此轮没有生产代码变更，无需重启服务，8080 监听保持运行。
