# 2026-10-20 发版验收

本轮对 TODO 修复及同批 MCP/审计/故障模拟/范围读取进行自动化与隔离验证。不同层次的证据分别记录；上一版[目标机器待验项](acceptance-2026-10-19.md)继续保留。

正式版 [0.1.31 / build-da127ea9ba1a](https://github.com/godstealer/http-capture/releases/tag/build-da127ea9ba1a)，来源 `da127ea9ba1a27eeb6476bb3ebad81ec46f1bbd1`；[Check #31](https://github.com/godstealer/http-capture/actions/runs/39426567394) 三系统及前端全部通过，[Release #26](https://github.com/godstealer/http-capture/actions/runs/39427236891) 四平台构建、各平台 10 项桌面更新生命周期测试和公开发布全部成功。公开 22 个附件已下载独立核验，17 项 SHA256 一致，五个更新包的正文及版本注释签名公钥验证通过，兼容更新清单一致。

## 已完成本地验证

- [x] `transport-api`、`transport-httpcloak`、`capture-core`、`capture-mcp`、`capture-service` 的完整回归，包括真实 stdio 子进程；系统凭据库写入与可选 TUN helper 的既有 ignored 项不计为通过。
- [x] 来源识别 22 项单元和 5 项集成，包括 16 条连接突发、IPv4/IPv6、H2 多路流、WebSocket、取消及不冒认 TUN helper。
- [x] Go helper 完整 20 项；此前定向 100 轮通过。覆盖 H1/H2、无 ALPN、HTTP CONNECT/SOCKS5、证书拒绝、Cookie、长度头和取消，不以任意错误重发 POST。
- [x] 独立 helper 使用 Chrome 和 Firefox 预设分别只读 GET 用户提供的 ANA 公开 JavaScript 链接，两次均为 200 / HTTP/2 / ALPN h2 / TLS 1.3；正文均为 4659 字节且 SHA256 一致。未重放原业务 POST，不代表鉴权或订票搜索验收通过。
- [x] 外部 Node stdio 夹具在固定 Node 环境下连续 30 轮、共 90 次子进程用例通过；不把本地重复回归替代跨平台 CI。
- [x] 完整前端 36 个回归脚本、TypeScript、国际化和生产构建。独立审查后补修窄屏详情工具栏换行，相关定向回归和构建通过。
- [x] 桌面及控制服务含测试代码编译通过。
- [x] 发布签名、不可变附件、版本来源及兼容更新频道 fixtures 34 项通过。

## 发布门禁与原生验证

本机桌面 updater 的 10 项测试程序编译和链接成功，但曾被 Windows 拒绝启动（os error 5，never executed）；没有修改系统安全设置，该本地尝试不计通过。随后 Release #26 在 Windows、Linux、macOS Intel / ARM 各自执行 `cargo test --release --locked -p http-capture-desktop --bin http-capture app_update::tests`，每组 10 项全部通过；四组安装包均在测试通过后才收集并公开发布。

首次源 Check #29 的 Windows、Linux、前端全部通过，macOS 的新增审计分类测试在故障端点等待超过 SDK 的 5 秒预算，因此没有发布该候选。原用例使用绑定但未监听的 socket，不能保证各平台立即拒绝连接。已改为测试持有本地监听器，确认接收完整 GET 后明确返回畸形 HTTP 响应，稳定验证真实发送失败及其审计分类；不增加 SDK 超时、不修改生产逻辑，原断言保留并补充没有有效响应的检查。修复后的定向测试及完整 MCP 43 项通过，使用新来源提交重新进行完整 CI。

第二次 Check #30 的 macOS、Linux、前端全部通过，包括上述审计测试；Windows 的既有外部 Node stdio 用例在首次初始化超过 15 秒，仍未发布。此超时在本地未复现，不能据此声称已确定生产管道或某个 Node 版本的问题。Rust 检查原先依赖 runner 预装 Node，现显式使用与前端/发布相同的 Node 22 并输出版本；测试补充有界、仅包含启动和协议方法/ID 的诊断文件，协议 stdout 不混入诊断，生产逻辑、无 shell 调用和原 15 秒预算不变。外部 Node stdio 夹具本地连续 30 轮、90 次子进程用例通过，最终 Check #31 的 Windows、Linux、macOS 和前端均通过；前两次未通过候选仍未发布。

- [x] 同一来源提交 `da127ea9ba1a27eeb6476bb3ebad81ec46f1bbd1` 的[Check #31](https://github.com/godstealer/http-capture/actions/runs/39426567394) 三系统和前端全部通过。
- [x] [Release #26](https://github.com/godstealer/http-capture/actions/runs/39427236891) 的 Windows、Linux、macOS Intel / ARM 四组桌面更新生命周期测试各 10 项全部通过。
- [x] 四组构建和[0.1.31 正式发布](https://github.com/godstealer/http-capture/releases/tag/build-da127ea9ba1a)完成；公开 22 个附件已独立下载，17 项 SHA256、五个更新包正文/版本注释签名及兼容频道一致性核验通过。

## 隔离浏览器检查

使用当前生产前端与独立内存接口模拟，不连接用户开发内核，不修改系统代理。浏览器检查补充组件回归，不能代替真实安装包、真实业务请求和操作系统默认浏览器。

- [x] 抓包详情响应替换入口跳转到匹配 URL/方法规则，说明仅对后续请求生效；修改状态码及 CodeMirror 正文后放行，等待项消失。
- [x] 600px 上游图标可直达已选中的上游设置页；360px 来源详情的响应替换按钮换行后可见。没有修改用户代理。
- [x] 便携版模拟 driver 下显示手动更新说明；固定 Releases 下载动作已发起，打开失败时显示错误、可选取地址与复制按钮。组件回归验证复制调用；实际 Linux 系统剪贴板与默认浏览器仍待目标机器。
- [x] MCP 审计展示成功/未确认/失败/拒绝及说明；故障规则改为 1200ms 后保存显示同步成功；迁移预览可选择“模拟规则”，未执行真实导入。

截图保存在本机 `.local/todo-response-360-20261020.png`、`.local/todo-upstream-narrow-20261020.png` 和 `.local/todo-update-fallback-20261020.png`；全部为模拟数据，不包含用户抓包内容。

## 继续保留的目标机器验收

- [ ] Linux 普通用户权限下浏览器实际进程识别；无权限、进程退出或超时仍保持未知，不依据网站推断。
- [ ] ANA 原业务请求和订票搜索，鉴权、网站状态及业务结果分别判断。
- [ ] 四平台真实安装、保存现有数据、跨版本更新与重启；Linux portable 实际打开默认浏览器及失败复制。
- [ ] 实际 AI 客户端的 HTTP/stdio 工具与资源发现、操作确认和安装版退出。
- [ ] 真网页的故障模拟、取消、共享固定窗口及多窗口冲突；规则导入后禁用。
- [ ] 大正文分段拼接摘要、并发峰值内存、临时文件和完整下载/保存；全链路低内存 P0 继续未完成。
