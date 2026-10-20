# 使用反馈修复清单（2026-10-20）

来源：用户提供的两页 TODO.pdf。截图包含 Linux 便携版 0.1.19；当前正式版为 [0.1.31](https://github.com/godstealer/http-capture/releases/tag/build-da127ea9ba1a)。以下按实际发布来源和验证证据复核，不把旧截图直接视为新版已经修复。

本轮五项代码修复已随 0.1.31 正式发布，0.1.28 安装包不包含这些修复。正式版 [0.1.31 / build-da127ea9ba1a](https://github.com/godstealer/http-capture/releases/tag/build-da127ea9ba1a)，来源 `da127ea9ba1a27eeb6476bb3ebad81ec46f1bbd1`；[Check #31](https://github.com/godstealer/http-capture/actions/runs/39426567394) 三系统及前端全部通过，[Release #26](https://github.com/godstealer/http-capture/actions/runs/39427236891) 四平台构建、各平台 10 项桌面更新生命周期测试和公开发布全部成功。公开 22 个附件已下载独立核验，17 项 SHA256 一致，五个更新包的正文及版本注释签名公钥验证通过，兼容更新清单一致。eCapture 只是另外的可行性评估；同批 MCP 契约、操作审计、故障模拟和正文范围读取改动一并交付。

| 项目 | 用户反馈 | 当前进度 |
| --- | --- | --- |
| 来源应用 | 同一网页的部分请求显示 chrome，部分显示“未识别应用” | 已修复：超过 4 个并发查询时在总计 1 秒内排队，不再立即放弃；连接分类共用有界策略 |
| ANA 请求 | httpcloak Firefox 发送时报 ALPN mismatch: expected h2, got http/1.1 | 已修复默认协议选择：服务端协商 H1 时，在发送 HTTP 数据前移交同一条已验证 TLS 连接；Chrome/Firefox 对 ANA 公开 JS 各 200/H2/TLS1.3；原业务 POST 待验 |
| 响应替换 | 找不到替换响应功能 | 已新增抓包详情“拦截替换响应”按钮，保留右键入口；提交按钮明确为“替换响应并放行” |
| 小屏代理入口 | 小屏幕不显示上游代理按钮 | 已修复：窄屏压缩为图标按钮，保留点击、状态提示及无障碍名称 |
| 便携版更新 | 手动更新提示需要一个下载链接 | 已补下载最新版与失败后的复制地址；区分“此安装方式需手动更新”和“更新源未配置” |

## 行为与边界

- 来源识别仍按本机 TCP 连接端点查进程，不按域名、相同网站或旧 PID 推测。等待、取消和超时均有界；查询线程实际结束前不释放并发名额，避免积压无限增长。权限不足、进程退出或超时仍显示未知；macOS 原生识别、UDP/QUIC 和 TUN 原始进程传递未因此完成。历史未知记录不追溯改写，需新建连接观察。
- httpcloak 默认浏览器预设依据 ALPN 兼容 H2/H1；只接受协议库明确的、发送 HTTP 数据前的协商结果，复用同一 TLS 连接及原 HTTP CONNECT/SOCKS5 隧道。任意响应读取或网络错误不触发跨协议重发 POST，不进入库的 Auto/H3 路径。显式 H2 和包含 H2 的导入 ClientHello 保持严格约束，不自动改写导入的 ALPN。
- H1 按原位置合并 Cookie、校验并规范化唯一的 Content-Length，拒绝不匹配长度和不支持的 Transfer-Encoding；移交 H1 后保留压缩正文的真实编码及长度头，不把未解压字节标为已解压。
- “拦截替换响应”针对后续匹配请求建立拦截，不修改已完成的历史响应，也不自动重发选中请求。内核断开和 WebSocket 场景显示不可用原因。
- 便携版下载入口固定指向本仓库 Releases 最新版；原生调用不接收任意网址。默认浏览器启动最多等待 2 秒，仍运行则异步回收子进程，不等待浏览器关闭、不终止浏览器。保留更新签名和 TLS 检查，未将手动更新伪装成应用内安装。

## 验证记录

已通过：

- 来源识别相关 22 项单元测试和 5 项真实代理集成测试，覆盖并发排队、超时/取消不启动迟到查询、实际 IPv4/IPv6 子进程、16 条连接突发、H2 多路流、WebSocket 及不冒认 TUN helper。
- Go helper 完整 20 项测试；协商、Cookie、代理及取消等定向用例连续 100 轮通过。覆盖 Chrome/Firefox 的 H1/无 ALPN 单次握手与 POST、H2 字段边界、HTTP CONNECT/SOCKS5、证书拒绝、压缩字节与头部一致、读取错误不跨协议重发及 Content-Length 大小写。
- 完整前端回归、TypeScript、国际化检查和生产构建。新增组件回归覆盖响应替换入口不重发历史请求、360–1440 px 上游入口规则、下载链接的浏览器/原生边界、失败复制、键盘焦点和更新忙碌状态。

桌面 `cargo check --locked --offline -p http-capture-desktop --tests` 通过，包含更新模块的 10 项测试代码。本机 `cargo test ... app_update::tests` 曾编译及链接成功但被 Windows 拒绝启动（`never executed / os error 5`）；该本地结果仍不计通过，也没有更改系统安全设置。随后同一发布来源在 Windows、Linux、macOS Intel / ARM 四个平台的 Release runner 中各自执行并通过全部 10 项原生更新生命周期测试，发布门禁已补齐。

待目标环境人工验收（延续用户暂缓安排）：

- [ ] Linux 实际权限与浏览器高并发下的来源识别，新建连接复核；无法识别时确认原因。
- [ ] 使用原 ANA 请求确认默认浏览器预设不再出现此次 ALPN 错误；网站鉴权、业务返回和搜索结果另行判断。本轮未向 ANA 重发业务 POST。
- [ ] 安装版的小屏上游按钮、抓包详情入口、后续响应替换操作。
- [ ] Linux 解包 portable 的手动更新提示、默认浏览器下载链接及失败后的复制；AppImage/Deb 更新方式不被误标。

追加验收：完整内核/传输/MCP/控制服务及真实 stdio 子进程回归通过。独立 helper 使用 Chrome 和 Firefox 预设分别只读 GET 用户提供的 ANA 公开 JavaScript 链接，两次均为 200 / HTTP/2 / ALPN h2 / TLS 1.3；正文均为 4659 字节且 SHA256 一致。未重放原业务 POST，不代表鉴权或订票搜索验收通过。外部 Node stdio 夹具在固定 Node 环境下连续 30 轮、共 90 次子进程用例通过；不把本地重复回归替代跨平台 CI。隔离生产界面已点击响应替换、修改后放行、360/600px 上游入口、审计结果、故障规则保存及模拟规则迁移预览；便携版更新使用模拟原生 driver 检查下载和失败恢复，不等于 Linux 默认浏览器实测。细节见[本次验收](acceptance-2026-10-20.md)。

本轮没有重启用户正在调试的旧内核。正式发布状态以同一来源提交的 Check #31、Release #26 和公开附件独立核验为据；上方人工验收项继续保留。
