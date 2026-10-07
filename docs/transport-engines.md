# 独立发送引擎与 httpcloak

## 已完成的拆分

- `crates/transport-api`：请求、响应、TLS 数据模型和通用 SendEngine 契约；不依赖抓包、数据库、脚本运行时或桌面界面。请求的脚本类型是泛型，传输层不执行脚本。
- `crates/browser-profiles`：UA 主版本解析、显式版本、latest、可用版本校验；wreq 预设目录可选编译。
- `crates/transport-wreq`：wreq 发送、ClientHello 配置和响应流读取；不依赖 capture-core。
- `crates/transport-httpcloak`：独立进程 IPC 客户端；输入输出大小限制、取消后结束子进程。
- `helpers/httpcloak`：锁定 httpcloak v1.7.2 的 Go 程序，标准输入/输出 JSON，不监听本地控制端口。
- `capture-core`：注册引擎、执行脚本/拦截、规范化请求、调用独立传输模块。native/h2/h3 暂仍位于 core，尚未迁入单独 transport-native crate。

## 构建与选择

Windows：运行 `scripts/build-httpcloak.ps1`（Go 1.26+，优先使用项目内便携 Go）。输出到 `.local/httpcloak/`，同时生成实际能力清单。Go 依赖版本和校验和保存在 helpers/httpcloak/go.mod、go.sum；构建使用 -mod=readonly。

其他系统可以在 helpers/httpcloak 下执行 `go build -mod=readonly -o http-capture-httpcloak .`，然后执行 `./http-capture-httpcloak --capabilities > http-capture-httpcloak.json`。

用 `HTTP_CAPTURE_HTTPCLOAK` 指定辅助程序的绝对路径；旁边必须有同名 .json 能力清单。桌面打包时可置于主程序旁。开发环境自动发现 `.local/httpcloak/`。本轮未重新生成桌面安装包，也未配置跨平台自动打包辅助程序。

重启服务后，在请求 TLS 页选择 httpcloak、Chrome、浏览器 TLS 版本。默认跟随 UA，无对应 UA 时选择当前引擎可用最大版本；明确指定 150 不会修改 UA 152。当前清单 Chrome 143–152，Firefox 133、148，使用 Windows 平台预设，未按本机操作系统自动切换预设。

## 边界与后续任务

原生 h3 引擎更新（2026-10-01）：已支持 SOCKS5 UDP ASSOCIATE 出站及代理端目标 DNS，外部 cloudflare-quic.com 经本机 SOCKS5 返回 200 / HTTP3 / TLS1.3。HTTP CONNECT 不支持此路径；httpcloak 的 H3 能力仍未接入。详情见 [实测记录](tls-components-test.md)。

- HTTPS 当前强制 HTTP/2，明文 HTTP 使用 HTTP/1.1；HTTP/3、HTTPS 的自动 h1 回退尚未接入此适配器。
- 使用 ExactHeaders 保留常规字段的有序列表，包括交错同名字段。Host 和正文长度仍由 core 根据 URL/正文规范化，协议伪头使用预设。
- 响应不自动跟随重定向，不使用浏览器预设替换 UA；不共享请求之间的 Cookie 或连接。
- 响应字段为协议库解析结果，不标记为原始线上顺序。库会流式解压常见压缩响应，适配器移除已解码的 Content-Encoding 并更新长度；此路径不保留原压缩字节。
- httpcloak 支持按请求选择 HTTP / SOCKS5 上游代理，凭据通过 stdin 传递，不写入请求记录或命令行；拒绝后不回退直连，并校验代理解析地址避免监听回环。自定义 TLS 高级覆盖、显式伪头列表暂不支持，明确报错。
- TLS 版本、协商密码套件、证书链来自验证回调，保持默认信任校验。握手广告字段未采集的值不伪造。
- 自定义 JSON 预设编辑/导入尚未接入；当前支持库内已实现的 Chrome 152，不以改版本名冒充新版本。

## 验证（2026-09-29）

- 完整 capture-core 默认构建回归通过（系统凭据写入和 TUN 集成的两个显式 ignored 测试未运行）。
- browser-replay 引擎/保序回归通过，Rust→Go 集成验证 UA 152 与显式 TLS 150 独立、999 被拒绝。
- Go 测试验证交错重复字段顺序、UA 保留、不注入 client hints、不跟随 302、gzip 结果及长度处理、不受信任证书拒绝。
- 前端生产构建和国际化检查通过。
- httpcloak Chrome 152 对 httpbin 实测 200 / HTTP2 / TLS1.2，UA 152 保留，返回 3 张证书。
- tls.peet.ws 上 Chrome 150、152 均超时，未宣称该站点指纹回显通过。跨平台实际运行尚未验证。

最终 API 联调：重启开发服务并恢复 127.0.0.1:8080；通过前端 /__capture/replay 路径，UA 152 + auto 实际选择 chrome-152-windows，UA 152 + 150 实际选择 chrome-150-windows，均返回 HTTP200 / HTTP2 / TLS1.2，UA 保留且证书链 3 张。修复并覆盖了无 DNS 名称证书返回 null 的 IPC 兼容问题。桌面端 browser-replay 编译检查通过。

## tls.peet.ws 跑通记录（2026-09-30）

后续更新：wreq 也已支持显式 HTTP / SOCKS5 上游配置，沿用 core 的代理地址解析及监听回环校验；禁用环境代理自动继承，SOCKS5 使用远端 DNS。Chrome149、Firefox151 分别通过两种代理实测 tls.peet.ws，四次均 200 / HTTP2、UA 保留。认证和拒绝不回退测试通过。完整记录见 [组件实测](tls-components-test.md)。

本机直连目标超时/重置，但系统本地代理 127.0.0.1:7897 可达。新增 httpcloak send_via 及 Go ProxyConfig，支持 HTTP CONNECT 和 SOCKS5，不自动继承系统代理，仍由请求显式选定代理配置。

辅助进程通过该代理，Chrome 152、150 均访问 https://tls.peet.ws/api/all 成功：200、HTTP2、TLS1.3、UA 152 保留。152 回显 JA4 `t13d1517h2_8daaf6152771_cb7bf5808d99`，150 为 `t13d1516h2_8daaf6152771_806a8c22fdea`。这些结果证明目标可达且选用预设有差异，不代表对全部浏览器行为的保真认证。

新集成测试覆盖 CONNECT 目标、Basic 代理认证、407 失败、不泄露凭据；原有上游代理测试全部通过。

最终部署验证：服务已更新、127.0.0.1:8080 监听已恢复，保存了命名代理‘系统本地代理 7897’。经前端 /__capture/replay 实际发送 auto（152）和显式 150 到 tls.peet.ws，均 200 / HTTP2 / TLS1.3，UA 保持 152。抓包全局上游保持原设置；需要在手动请求中选择该命名代理。
