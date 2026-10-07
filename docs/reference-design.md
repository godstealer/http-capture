# 抓包与浏览器重放的参考设计

## 参考来源

- HTTP Toolkit 的 Mockttp 类型定义：https://github.com/httptoolkit/mockttp/blob/main/src/types.ts
- httpcloak：https://github.com/sardanioss/httpcloak

参考日期：2026-09-22。这里记录设计取舍，不复制上游实现，也没有引入两个项目的运行时依赖。

## 已采用

Mockttp 区分解析后的字段、原始字段序列以及正文数据。我们继续用有序 Header 数组保存大小写和交错重复字段，用 Base64 保存原始 HTTP/1 头部；GUI 提供解析头部、原始头部、正文文本、正文 Base64 四种视图。二进制或压缩正文不会为了展示而覆盖存储数据。正文已解除 chunked 分帧，不代表整段原始 TCP 流。

捕获记录与发出的请求必须分开理解：Host、Content-Length、Connection 等可能因代理转发而调整，详情提供转发说明。原始请求头是客户端发给代理的字节；原始响应头是上游发给代理的字节，均不冒充代理输出快照。

httpcloak 将 TLS、HTTP/2 和头部顺序纳入浏览器配置。我们保留 RequestDraft.headers 与 TlsProfile 的独立边界；选择 TLS 预设不能覆盖用户编辑的 HTTP 字段。不能表达交错重复字段的浏览器后端必须报错，不得静默合并。

## 后续实现与验收

1. 生命周期前移到请求头到达：receiving、forwarding、completed、failed、aborted；使用单调时钟记录正文接收、连接、TLS、首字节与完成阶段。当前记录从正文接收完成后开始，耗时不含客户端上传阶段。
2. 浏览器配置固定版本，导入配置必须校验后端能力；不仅比较 JA3，还需在本地 TLS 服务端验证 ClientHello 扩展、ALPN，以及 HTTP/2 SETTINGS、伪头和普通字段顺序。
3. wreq 与 httpcloak 并存，通过独立 SendEngine 接口接入；每次请求携带 engine 字段动态选择，TLS 预设单独配置。对比结果用于说明各引擎能力，不淘汰其他后端。Go sidecar 接入后注册为 httpcloak。当前默认构建仅启用 native；浏览器功能仍未完成编译和线缆级验证，不能宣称 Chrome/Firefox 完全一致。
4. 为发送后的头部添加独立快照，避免用捕获输入或预设配置推断实际输出。HTTP/2 不适用 HTTP/1 的大小写和原始文本报文概念。

保留 Rust/Tauri 架构。HTTP Toolkit 用于参考代理生命周期和数据呈现；httpcloak 用于参考浏览器配置和后端验收。

## 多引擎接口落地

transport.rs 提供 SendEngine 与 SendEngines 注册表。内核状态返回引擎列表、可用性、支持的预设和不可用原因；UI 按此列表选择。未知或未启用的引擎报错，不静默回退；请求记录保留 engine 字段。旧记录缺省为 native。切换引擎时保留兼容的 TLS 配置，不兼容时切到新引擎的默认预设。抓包转发当前仍使用 native；这里的动态选择作用于编辑后的重放。

当前验证：engines.rs 验证同一 TLS 预设在并发请求中选择不同注册后端、拒绝不可用后端；fidelity.rs 提供共用 HTTP/1 线上观察夹具，检查头部大小写与顺序、二进制正文、禁止自动重定向及不修改草稿。native 为默认基线，wreq 在 browser-replay 构建下运行同一夹具。httpcloak 尚未接入，ClientHello 与 HTTP/2 专项夹具尚待补齐；未运行项不能视为通过。
