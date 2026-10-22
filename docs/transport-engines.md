# 独立发送引擎与 httpcloak

当前开发状态（2026-10-22）：httpcloak 已升级到 **v1.7.3**，真实能力清单支持 Chrome **143–155**、Firefox **133、148**，本增量已随 0.1.34 四平台发布；0.1.33 附带的仍为 v1.7.2。wreq/httpcloak 已接入 HTTP/SOCKS5，httpcloak 默认浏览器预设按 ALPN 兼容 H2/H1。原生 H3 经 SOCKS5 UDP 到 tls3.peet.ws 有既有实测，TUN H3 接管仍未验收且暂缓。以下按日期保留历史验证；早期失败或不支持项不代表当前能力，真实安装和业务流程仍分别验收。


## 已完成的拆分

- `crates/transport-api`：请求、响应、TLS 数据模型和通用 SendEngine 契约；不依赖抓包、数据库、脚本运行时或桌面界面。请求的脚本类型是泛型，传输层不执行脚本。
- `crates/browser-profiles`：UA 主版本解析、显式版本、latest、可用版本校验；wreq 预设目录可选编译。
- `crates/transport-wreq`：wreq 发送、ClientHello 配置和响应流读取；不依赖 capture-core。
- `crates/transport-httpcloak`：独立进程 IPC 客户端；输入输出大小限制、取消后结束子进程。
- `helpers/httpcloak`：锁定 httpcloak v1.7.3 的 Go 程序，标准输入/输出 JSON，不监听本地控制端口。
- `capture-core`：注册引擎、执行脚本/拦截、规范化请求、调用独立传输模块。native/h2/h3 暂仍位于 core，尚未迁入单独 transport-native crate。

## 构建与选择

Windows：运行 `scripts/build-httpcloak.ps1`（Go 1.26+，优先使用项目内便携 Go）。输出到 `.local/httpcloak/`，同时生成实际能力清单。Go 依赖版本和校验和保存在 helpers/httpcloak/go.mod、go.sum；构建使用 -mod=readonly。

其他系统可以在 helpers/httpcloak 下执行 `go build -mod=readonly -o http-capture-httpcloak .`，然后执行 `./http-capture-httpcloak --capabilities > http-capture-httpcloak.json`。

用 `HTTP_CAPTURE_HTTPCLOAK` 指定辅助程序的绝对路径；旁边必须有同名 .json 能力清单。桌面打包时可置于主程序旁。开发环境自动发现 `.local/httpcloak/`。正式发布已在 Windows、Linux、macOS Intel / ARM 的原生 runner 构建并随安装包打包辅助程序及能力清单，不依赖用户源码目录。

重启服务后，在请求 TLS 页选择 httpcloak、Chrome、浏览器 TLS 版本。默认跟随 UA，无对应 UA 时选择当前引擎可用最大版本；明确指定 150 不会修改 UA 155。当前清单 Chrome 143–155，Firefox 133、148，使用 Windows 平台预设，未按本机操作系统自动切换预设。UA 是不支持的版本时会明确报错，不自动冒用其他版本。

## 更新 httpcloak

普通用户通过更新 HTTP Capture 安装包获得新版 helper，无需单独安装 Go 库。当前项目锁定 `httpcloak v1.7.3`，没有独立自动更新库的功能。[上游固定版本更新记录](https://github.com/sardanioss/httpcloak/blob/v1.7.3/CHANGELOG.md) 包含 Chrome 153–155 预设；本轮同时更新 `sardanioss/net` 至 v1.2.12、`sardanioss/quic-go` 至 v1.2.30，保留 Go 1.26.0 最低版本。升级不改变应用中由用户提供的 UA、Accept 等请求头。

维护者在 `helpers/httpcloak` 中检查可用版本，再固定目标版本升级。下面使用本轮已升级的 `v1.7.3` 演示，未来替换为经过评估的具体版本：

```sh
cd helpers/httpcloak
go list -m -versions github.com/sardanioss/httpcloak
go get github.com/sardanioss/httpcloak@v1.7.3
go mod tidy
go mod verify
go test -mod=readonly ./...
```

检查并提交 `go.mod`、`go.sum` 的依赖变化；流程见 [Go 官方升级依赖说明](https://go.dev/doc/modules/managing-dependencies#upgrading-or-downgrading-a-dependency)。回归重点包括 TLS/ALPN 的 H2/H1 协商与导入 ClientHello 严格约束、ExactHeaders 的顺序及重复字段、H2 Cookie 边界与 H1 合并、HTTP CONNECT/SOCKS5 不绕过代理、取消释放连接，以及压缩或二进制正文的字节与头部一致性。网络或响应错误不得导致 POST 跨协议重发。

回归通过后，停止使用该 helper 的内核，按上节方式重建 helper 并重新生成同名 `.json` 能力清单，再重启内核读取新清单；不要只替换二进制或只编辑 JSON。正式发布 CI 在 Windows、Linux、macOS Intel / ARM 分别测试、构建并随安装包打包二者。浏览器预设来自编译后的库；`latest` 只代表该库支持的最新版本，修改 User-Agent 不会下载或生成新的浏览器指纹。

## 边界与后续任务

2026-10-21 v1.7.3 升级验证：Go 模块校验、helper 全部回归通过；新增 153/154/155 继承保护和 155 的实际 H2 Cookie/ALPN H1-H2 测试。Windows 本地 helper 已重建，其构建信息确认 v1.7.3，能力清单最高版本 155。Rust→Go 四项集成测试通过，覆盖 UA 155 跟随、latest、显式 155、显式 153/150，以及 UA/Accept/交错重复字段保持。四平台安装包、每个平台 10 项原生更新测试及公开附件签名核验已通过；实际运行和业务网站验收仍待处理。

原生 h3 引擎更新（2026-10-01）：已支持 SOCKS5 UDP ASSOCIATE 出站及代理端目标 DNS，外部 cloudflare-quic.com 经本机 SOCKS5 返回 200 / HTTP3 / TLS1.3。HTTP CONNECT 不支持此路径；httpcloak 的 H3 能力仍未接入。详情见 [实测记录](tls-components-test.md)。

- 已发布的 0.1.28 中 HTTPS 默认强制 HTTP/2，明文 HTTP 使用 HTTP/1.1。0.1.31 已发布修复，为默认浏览器预设补充 ALPN 协商：服务端选择 H1 或无 ALPN 时，在发送 HTTP 数据前将同一已验证 TLS 连接交给 H1，沿用原 HTTP CONNECT/SOCKS5 隧道，不重新握手或因普通网络/响应错误跨协议重发。显式 H2 和包含 H2 的导入 ClientHello 保持严格约束；不启用库的 Auto/H3 路径，HTTP/3 仍未接入此适配器。详见 [TODO 修复记录](todo-feedback-2026-10-20.md)。
- 使用 ExactHeaders 保留常规字段的有序列表，包括交错同名字段。Host 和正文长度仍由 core 根据 URL/正文规范化，协议伪头使用预设。
- 响应不自动跟随重定向，不使用浏览器预设替换 UA；不共享请求之间的 Cookie 或连接。
- 响应字段为协议库解析结果，不标记为原始线上顺序。库的常规路径解压常见压缩响应，适配器移除已解码的 Content-Encoding 并更新长度；此路径不保留原压缩字节。0.1.31 新增的同连接 H1 移交路径则返回原编码字节，保留相应 Content-Encoding 和长度头，不声明已经解压。
- httpcloak 支持按请求选择 HTTP / SOCKS5 上游代理，凭据通过 stdin 传递，不写入请求记录或命令行；拒绝后不回退直连，并校验代理解析地址避免监听回环。自定义 TLS 高级覆盖、显式伪头列表暂不支持，明确报错。
- TLS 版本、协商密码套件、证书链来自验证回调，保持默认信任校验。握手广告字段未采集的值不伪造。
- 自定义 JSON 预设编辑/导入尚未接入；当前支持库内已实现的 Chrome 155，不以改版本名冒充新版本。

## 0.1.31 验证补充（2026-10-20）

正式版 [0.1.31 / build-da127ea9ba1a](https://github.com/godstealer/http-capture/releases/tag/build-da127ea9ba1a)，来源 `da127ea9ba1a27eeb6476bb3ebad81ec46f1bbd1`；[Check #31](https://github.com/godstealer/http-capture/actions/runs/39426567394) 三系统及前端全部通过，[Release #26](https://github.com/godstealer/http-capture/actions/runs/39427236891) 四平台构建、各平台 10 项桌面更新生命周期测试和公开发布全部成功。公开 22 个附件已下载独立核验，17 项 SHA256 一致，五个更新包的正文及版本注释签名公钥验证通过，兼容更新清单一致。

独立 helper 使用 Chrome 和 Firefox 预设分别只读 GET 用户提供的 ANA 公开 JavaScript 链接，两次均为 200 / HTTP/2 / ALPN h2 / TLS 1.3；正文均为 4659 字节且 SHA256 一致。未重放原业务 POST，不代表鉴权或订票搜索验收通过。

本版仍锁定 httpcloak v1.7.2，未升级到上游 v1.7.3；后续升级需要独立兼容回归。实际 ANA 业务 POST、完整浏览器行为保真和四平台安装体验仍保留待验，不以只读公开 JS 或构建成功代替。

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
