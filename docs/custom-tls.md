# 自定义请求 TLS

## 浏览器版本选择

Chrome / Firefox 按钮不再固定版本。选择 wreq 引擎及浏览器类型后，可以配置“浏览器 TLS 版本”：

- 跟随 User-Agent（默认）：读取对应浏览器的主版本号；无对应 UA 时使用库内最新预设。
- 使用库支持的最新版本：忽略 UA 中的版本选择预设。
- 指定版本：从后端实际支持的版本列表选择；不会修改 User-Agent 或其他请求头。

配置保存在 `tls.browserVersion`：省略或 `auto` 表示跟随 UA，`latest` 表示库内最新，数字字符串表示指定版本。版本同时选择浏览器的 TLS / HTTP2 预设。

当前锁定的 wreq-util 0.2.0 支持 Chrome 最高 149、Firefox 最高 151，版本中间存在缺口，界面只列出真实可用项。UA 为 Chrome 152 时自动模式明确报错，可以手动选择 149；目前不能选择尚未实现的 Chrome 150 / 152，也不会静默伪装成它们。重复 User-Agent 在自动模式下会报错，需明确选择版本。

需要以 browser-replay 特性构建服务（`npm run capture:browser` 或 `npm run desktop:browser`）。该特性控制 wreq；httpcloak 通过独立 Go helper 提供自己的浏览器预设，见 [发送引擎](transport-engines.md)。现有服务不会因修改源码自动切换为浏览器构建。

请求编辑页的 TLS 标签支持原生引擎（auto / native / h2 / h3）的独立配置，保存和重开标签时随 RequestDraft 持久化。留空使用默认值。

- TLS 版本：自动、仅 1.2、仅 1.3。HTTP/3 仅支持 1.3。
- Cipher suites：IANA 名称，冒号、逗号或空白分隔，保留提供顺序。例如 `TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384`。
- Supported groups：`X25519:secp256r1:secp384r1`，按输入顺序提供。支持 P-256、prime256v1、P-384 别名。
- ALPN 跟随发送引擎：auto 提供 h2 / http/1.1，native 使用 http/1.1，h2 使用 h2，h3 使用 h3。

TLS 1.2 密码套件支持 ECDHE_ECDSA / ECDHE_RSA 与 AES_128_GCM_SHA256、AES_256_GCM_SHA384、CHACHA20_POLY1305_SHA256 的组合（完整名称示例：`TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256`）。TLS 1.3 支持 TLS_AES_128_GCM_SHA256、TLS_AES_256_GCM_SHA384、TLS_CHACHA20_POLY1305_SHA256。

参数构造逐请求生效，不改变进程默认配置或抓包客户端 TLS。实际协商结果由客户端提供项和服务器能力共同决定，可在响应 TLS 详情中查看。证书链和目标主机名校验保持开启。

未知参数、重复条目、无可用密码套件、HTTP/3 配 TLS 1.2 等情况报错，不静默降级。原生引擎不支持 GREASE、自定义签名算法、扩展排列或完整浏览器指纹模拟。wreq 浏览器预设仍需要 browser-replay 构建；其 cipher list 使用 BoringSSL 语法，不应与原生 IANA 列表混用。此次新增 version 字段不适用于 wreq，指定时明确拒绝。httpcloak 已接入浏览器预设，但不支持上述原生 TLS 高级覆盖，传入不支持配置时明确拒绝。SNI 自定义、mTLS、任意 ALPN 与 ClientHello 扩展编辑未实现。

实现参考 [rustls ClientConfig](https://docs.rs/rustls/0.23.45/rustls/client/struct.ClientConfig.html) 和项目锁定版本的本地源码。配置入口：crates/capture-core/src/tls_config.rs。

验证：tls_config 单元测试；custom_tls 本地真实握手测试（TLS 1.2/1.3、指定密码套件、不受信任证书拒绝）；capture、engines、multiplex 回归测试；前端构建和国际化字典检查。不代表浏览器指纹一致性测试，也未进行 GUI 点击验收。

更新：新增 httpcloak 引擎支持 Chrome 143–152。上文 Chrome 149 上限仅指 wreq；版本列表按所选引擎提供。详见 [transport-engines.md](transport-engines.md)。
