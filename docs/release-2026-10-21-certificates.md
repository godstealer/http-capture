# HTTP Capture 0.1.34 正式更新：证书管理与 Chrome 155（2026-10-21；Mac 补发于 2026-10-22）

正式版 [0.1.34 / build-20f9137572e0](https://github.com/godstealer/http-capture/releases/tag/build-20f9137572e0) 四平台已发布，应用源码 `20f9137572e00c4faa06d6290b94ac01f10c2105`，版本沿用 [Check #34](https://github.com/godstealer/http-capture/actions/runs/39621202898)；Mac 源码检查现已通过。[Windows/Linux 构建](https://github.com/godstealer/http-capture/actions/runs/39625026369) 和 [Mac 双架构构建](https://github.com/godstealer/http-capture/actions/runs/39700840500) 均通过，每个平台 10 项原生更新测试通过。22 个公开附件、17 项 SHA256、五个更新包及版本注释签名、兼容更新频道一致性已独立核验；自动更新频道与 GitHub Latest 已推进至 0.1.34。

## 更新内容

- **三类证书统一管理**：新增“设置 → 证书管理”，分别管理自定义抓包 CA、mTLS 客户端身份和上游服务器信任；支持导入、指纹/签发者/有效期查看、显式启停和删除。
- **自定义抓包 CA**：导入 CA 证书和匹配私钥，确认切换后重启整个内核生效，支持中间 CA 证书链；保留原内置 CA 可切回，不自动安装或删除系统信任。
- **双向 TLS 客户端身份**：按精确域名/IP 和端口配置客户端证书与私钥，匹配目标的新连接使用该身份；同一目标避免启用冲突身份，Native/Auto/H2/H3、gRPC、WebSocket 和可选 wreq 已接入。
- **私有服务器信任**：导入 CA 公钥扩展上游信任，可限定域名与端口或作用于全部目标，继续验证证书链、有效期和域名；httpcloak 同样支持自定义信任 CA。
- **证书存储与编辑保护**：校验证书用途、有效期和证书私钥匹配；导入默认不生效，启用与切换需要明确确认。Windows 使用当前用户加密，其他系统限制文件权限；私钥不进入请求记录、草稿和全局配置导出，多个窗口/进程编辑使用版本与文件锁保护。
- **httpcloak 更新至 1.7.3**：加入真实 Chrome 153、154、155 预设，当前目录支持 Chrome 143–155。跟随 UA、latest 及显式版本选择使用库中实际预设，保留用户填写的请求头，继续支持旧版本选择。
- **设置入口更清楚**：顶部“代理设置”改为“设置”，各类别保留独立选项卡；证书用途有选中高亮，分别显示对应文件和目标输入，断开内核时禁止写入。

## 使用方式

顶部打开 **设置 → 证书管理**，先选择用途、填写名称并选择文件。客户端身份还需要精确域名/IP 和端口；服务器信任留空域名时适用于全部目标。导入后核对指纹并按需要启用；抓包 CA 则点击切换、确认并重启整个应用/内核。单纯停止/开始捕获不会重新加载 CA。

需要双向 TLS 的接口使用“客户端证书”；私有 CA 服务使用“服务器信任”；让浏览器信任抓包工具仍在“系统与证书”中安装公钥 CA。三者用途不同，详见[证书管理说明](certificates.md)。

HTTP 重放的 TLS 页选择 **httpcloak → Chrome → 155**，或 UA 为 Chrome/155 时使用跟随模式。`latest` 当前选择 155；选择 Firefox 不会改写 Chrome 请求头，混合配置由用户决定。更新后必须重启内核才能加载新 helper 和能力清单。

## 已完成的本地验证

- 完整 core/service/transport-httpcloak 回归、证书 API 认证/版本冲突/私钥隔离、跨进程持久化与导入校验通过。
- 独立本地 Native/Auto/H2/H3/wreq mTLS、CONNECT 抓包、中间 CA 完整证书链握手通过。
- Go 模块完整性校验、helper 全部回归通过；Chrome 155 的 H2 Cookie 字段、H1/H2 ALPN、ClientHello 导入、自定义根信任及旧版本兼容保留。
- Rust→Go 版本选择集成通过，覆盖自动/latest/显式 155、显式 153/150 和 UA/Accept/重复字段顺序保持。
- 45 项前端脚本、国际化、TypeScript/Vite 生产构建及桌面 browser-replay 含测试代码编译通过；内置浏览器已核对分类与表单布局。

## 当前边界与待验

- 当前支持 PEM/DER 证书和未加密 PEM 私钥；P12/PFX 与加密私钥直接导入仍未实现。普通非 CA 的自签名终端证书不能直接作为信任 CA 导入。
- httpcloak 1.7.3 仍没有本适配器所需的客户端身份接口，匹配已启用 mTLS 身份时明确拒绝发送，请使用其他支持的发送引擎；升级不等于新增 httpcloak mTLS。
- Windows 证书库绑定当前用户；macOS/Linux 使用受限本机文件，不宣称磁盘加密。证书库不参与全局迁移，跨设备需从原始文件重新导入。
- 安装包不包含可选 wreq；本文 wreq 验证针对启用 browser-replay 的开发构建。
- 原生文件选择、各系统信任操作、真实安装升级、跨设备和业务网站验收仍待用户方便时进行；P0 全链路低内存及 TUN H3 实机验收维持原清单状态。

## 发布记录

正式版 [0.1.34 / build-20f9137572e0](https://github.com/godstealer/http-capture/releases/tag/build-20f9137572e0) 四平台已发布，应用源码 `20f9137572e00c4faa06d6290b94ac01f10c2105`，版本沿用 [Check #34](https://github.com/godstealer/http-capture/actions/runs/39621202898)；Mac 源码检查现已通过。[Windows/Linux 构建](https://github.com/godstealer/http-capture/actions/runs/39625026369) 和 [Mac 双架构构建](https://github.com/godstealer/http-capture/actions/runs/39700840500) 均通过，每个平台 10 项原生更新测试通过。22 个公开附件、17 项 SHA256、五个更新包及版本注释签名、兼容更新频道一致性已独立核验；自动更新频道与 GitHub Latest 已推进至 0.1.34。

Windows/Linux 首发时间：`2026-10-21T15:47:45Z`；Mac 补发于 2026-10-22。Windows 下载 `windows-x64-HTTP-Capture_0.1.34_x64-setup.exe`；Mac 根据芯片选择 `macos-arm64` 或 `macos-x64` DMG；Linux 按需要选择 deb、AppImage 或 portable。完整自动更新已开放，自动化通过不替代上面的实机验收。

后续开发与待验项目见[功能路线图](roadmap.md)，打包与更新流程见[发布说明](releases.md)。
