# 局域网协同抓包

目标：PC 显示一次性配对二维码，Android 扫码、授权 VPN 后，将流量送到 PC 抓包列表。默认模式不要求安装抓包 CA；HTTPS 只显示连接与可见域名等元数据，正文解密必须由用户另行安装并信任 PC 的 CA。

证书边界：Android 7 起，面向新系统的应用默认可能不信任用户添加的 CA；Android 11 起，普通应用不能通过 `KeyChain.createInstallIntent()` 替用户安装 CA，只能提供公钥证书供用户到系统设置安装。扫码配对不能改变目标应用的信任策略。[Android 网络安全配置](https://developer.android.com/privacy-and-security/security-config)、[KeyChain 文档](https://developer.android.com/reference/android/security/KeyChain)。

配对二维码只包含协议版本、PC 局域网地址、接收端口、短期配对凭据和 PC 传输身份指纹，不包含 CA 私钥。PC 只在用户打开配对窗口时接受新设备；凭据首次使用后作废，断开设备时撤销会话。手机校验指纹后建立加密通道，VPN 转发通道必须排除在自身 VPN 路由外，避免回环。

实现清单（2026-10-28，开发分支代码已推送；尚未设备验收）：

- [x] 定义 v1 配对 URI，校验私有局域网 IPv4、端口、32 字节随机令牌、PC 身份指纹、版本及五分钟有效期；本地定向测试通过。
- [x] PC 独立 TLS 接收端绑定所选私有 IPv4；每次启动生成临时身份，扫码令牌五分钟有效、只可兑换一次，之后使用一小时会话凭据。会话可撤销，旧连接随之关闭。不会把无认证抓包代理开放成配对入口。
- [x] PC 设置页显示配对二维码、地址、连接状态和断开/关闭操作；接收端记录 `android-lan` 来源。设备名称已随桌面 0.1.82 发布，对应 Android 客户端仍为测试代码；更完整的 HTTP 会话尚未实现。
- [x] Android 内置扫码与 PC 地址/指纹确认；在固定 SPKI 指纹和 IP SAN 的 TLS 通道中兑换令牌。远程模式仅在配对完成、VPN 获系统授权后启动。
- [ ] TCP/UDP 完整转发：TCP 通过本机 CONNECT 适配器，UDP 通过本机 UDPGW 适配器，均在受保护的 TLS socket 中送到 PC，由 PC 建立上游 socket；同一 UDP 会话复用源端口。远程模式关闭手机直连 UDP。**Android 设备上的 DNS、UDP、QUIC 和长时间会话仍需验证与修复**。五秒心跳失败会停止 VPN，连接不回退到直连。
- [x] 默认不要求抓包 CA；远程 TCP 隧道不作 MITM，PC 只保存可见连接及首个 HTTP 请求元数据。HTTPS 正文和完整 HTTP 会话记录尚不可见；可选 CA 解密模式尚未实现。
- [ ] 验证：Rust 回环测试已覆盖正确/错误 PC 身份、一次性令牌、重放、心跳、TCP/UDP 透传、UDP 源端口复用和撤销；[桌面同源 Check](https://github.com/godstealer/http-capture/actions/runs/40322052848)、[Android 构建](https://github.com/godstealer/http-capture/actions/runs/40276020966)和[iOS 构建](https://github.com/godstealer/http-capture/actions/runs/40276020908)均成功。Android 模拟器与真机的扫码、VPN 回环、HTTP/HTTPS、DNS/UDP/QUIC、切网、断线及停止清理仍需端到端验收。

当前状态：安全配对与 TCP/UDP 转发代码已通过构建和回环测试，但尚无 Android 模拟器或真机端到端证据，不能宣称局域网协同抓包完成，也不发布移动安装包。

## 正文可用性契约（2026-11-16 开发增量）

正文是否记录与正文长度、请求是否完成分别表示。`bodyCapture` 可为 `request`、`response`、`originalRequest`、`originalResponse` 指定 `recorded` 或 `notRecorded`；`recorded` 仍需结合既有生命周期判断是否接收完毕。旧普通抓包缺少该字段时保留原语义；旧 `android-lan` 缺字段时一律保守视为未记录，显式 `recorded` 才能覆盖此回退，真实空正文仍允许读取为零字节。

新 PC 网关明确标记元数据记录；Store 的当前投影、旧 0 B 缓存、完整 Flow 与 MCP 都采用同一判定，不把占位字符串或过期正文引用当成完整实体。连接/可见头部仍可查看和搜索；对应正文的读取、下载、复制编辑/重放、cURL、HAR 与 MCP 组合请求在未记录时拒绝。请求与响应状态独立，已记录的响应不因请求未记录而失去只读能力。Android/iOS 的正文摘要、查看与复制入口同样检查状态。

会话 JSON 保留完整元数据与显式状态，未记录实体的兼容 `bodyBase64` 为空占位而不是长度证明。只要实际存在的实体中含 `notRecorded`，文件封装采用 `http-capture-session` version 2，让旧版读取器明确拒绝；全部实体已记录时继续使用 version 1。新导入器同时接受 1/2，并对旧 LAN 快照归一化。共享核心在同一只读事务内先扫描最小元数据选择版本，再逐条写出，版本与内容不会跨快照；不读取全部历史正文做预扫，也不推进暂停的 P0 大正文链路。

本批仅为开发增量，尚未发布新桌面或移动安装包。模型/Store 契约 12 项、受影响 Store 54 项（包含前述 Store 用例）、认证网关 6 项、MCP 43 项均在本机实际运行通过；Android 纯 Java 回归与 API36 离线编译通过。完整前端回归、类型检查、生产构建、4 项真实 HTTP 控制 API 回归及桌面含测试源码检查均已通过。集中跨平台 CI 与 iOS Swift Mac 编译结果待本批核对，不能据 Java 或 Rust 通过宣称完成设备验收。

## 设备名称（PC 0.1.82，Android 测试客户端）

Android 使用制造商/型号生成显示标签，不读取序列号、IMEI、MAC 或用户账户。标签独立于应用/进程信息，界面明确标为“设备名称（自报）”；来源应用仍显示未识别的远程客户端，导入文件仍提示来源未经本机验证，手动重放不继承设备标签。

二维码与 op1 一次性配对帧保持 v1。兑换会话后另建同样固定 SPKI、IP SAN 的 TLS 连接，发送 `op5 + sessionToken[32] + u16BE(nameUtf8.length) + nameUtf8`。标签为 1–128 字节，禁止控制/双向格式和行段分隔符；PC 接收完整字段后在同一锁内重验会话令牌与到期时间，才写入当前会话。元数据更新不撤销现有隧道；断开/替换会话会清空标签，状态查询隐藏过期标签。

旧 PC 返回不支持或提前关闭元数据连接时，Android 只有另行 op3 验证会话仍有效后才继续无名称配对。建立 TLS、身份校验与 VPN 回环保护失败，以及未知返回码、会话撤销/过期不能当作兼容性降级吞掉。配对阶段的会话校验不要求 VPN 已启动；VPN 已运行时仍保护远程 socket，原 VPN 心跳继续要求保护成功。

本次本机验证：6 项网关 Rust 回归、1 项来源 JSON 往返、Java 设备名称编码 79 项、生产方法模拟 socket 58 项及代理就绪门控 25 项通过；Android Java 编译、完整前端回归/构建、core/desktop 源码检查通过。[Android API36 双架构构建与模拟器烟测](https://github.com/godstealer/http-capture/actions/runs/43674526716) 已在 `94c1e4d` 上通过（`smokeStatus=ok`、`INSTRUMENTATION_CODE: -1`），原严格 TUN 来源断言保留。[iOS Mac 验证](https://github.com/godstealer/http-capture/actions/runs/43671382438) 在相关源码 `23e0687` 上通过模拟器启动及无签名 iPhone 编译，此后 iOS/Rust 源码未变；不将其记为新的同源 iOS 或真机验收。完整 HTTP 会话、HTTPS 可选解密和移动正式包均未完成。

## 本批移动构建复验

设备名称来源提交 `d7805842a2ca667229d9e27b408bde93b2918575` 的[桌面 Check](https://github.com/godstealer/http-capture/actions/runs/43668302959) 已通过；[Android 首轮](https://github.com/godstealer/http-capture/actions/runs/43668303018) 在 BoringSSL ARM64 链接时发现架构不兼容，[iOS 首轮](https://github.com/godstealer/http-capture/actions/runs/43668302978) 缺少 C++ 运行时符号。这两项未记为通过，修复后需核对新来源的全部验证。

Android 构建按每个 target 生成完整 NDK CMake wrapper，明确 ABI、API26 和 `c++_shared`，独立设置目标 C/C++ 编译器；两个 ABI 均打包对应 `libc++_shared.so`，检查 ELF Machine、DT_NEEDED 与 APK 内条目。Windows 的 Ninja generator 只使用原生 `ninja.exe`，不接受 `depot_tools/ninja.bat`，并覆盖旧的 make-program 缓存；开发环境需提供 CMake 和原生 Ninja。配置依据见 [Android NDK CMake](https://developer.android.com/ndk/guides/cmake)。iOS 应用和 Packet Tunnel 两个 target 均显式链接系统 `libc++`。没有降级 BoringSSL 或关闭 TLS 验证。

本机最小完整 BoringSSL SSL 共享库已在 NDK r30 下实际交叉链接为 Android ARM64 和 x86_64，`llvm-readelf` 核实两种架构及 `libc++_shared.so` 动态依赖；这不等于完整 APK、Linux NDK r27 CI 或设备运行通过。脚本语法、Shell 配置 mock 22 项与 PowerShell 配置 mock 47 项检查通过；当时完整移动构建与模拟器尚待修复批次 CI，最终通过结果见下文。原 CI 的精确 CMake 错位来源尚缺配置证据，因此不能仅归因“没有设置 CXX”；失败时另输出实际 CMake 配置诊断。

### 本轮最终结果（桌面 0.1.82）

来源 `94c1e4df961fecc68e50effeed509ebdda7e7e99` 的[同源 Check #82](https://github.com/godstealer/http-capture/actions/runs/43675127242) 与[四平台 Release](https://github.com/godstealer/http-capture/actions/runs/43675722681) 全部通过；每个平台 10 项原生更新生命周期测试、两种 Mac 应用代码签名检查通过。22 个公开附件已独立下载核验，17 项 SHA256、五个更新包签名及兼容更新频道一致性均通过，GitHub Latest 已推进至 0.1.82。[Android API36 双架构构建与模拟器烟测](https://github.com/godstealer/http-capture/actions/runs/43674526716) 已在 `94c1e4d` 上通过（`smokeStatus=ok`、`INSTRUMENTATION_CODE: -1`），原严格 TUN 来源断言保留。[iOS Mac 验证](https://github.com/godstealer/http-capture/actions/runs/43671382438) 在相关源码 `23e0687` 上通过模拟器启动及无签名 iPhone 编译，此后 iOS/Rust 源码未变；不将其记为新的同源 iOS 或真机验收。桌面包已正式公开，Android/iOS 仅保留测试构建；局域网完整 HTTP 会话和设备端到端仍未完成。

## 后续 HTTP 会话观察的顺序

- 正文可用性契约已接通为本地开发增量；先完成集中跨平台 CI，再进入首份 HTTP 头部观察。旧 `android-lan` 元数据不再被当作完整空正文，设备端到端与真实空体仍需分别验证。
- 再接入有界、只读的首份请求/最终响应头观察，保留原始 TCP 字节、背压和半关闭；观察失败或超限只停止观察，不能中断转发。
- 下一批增加正文 framing 计数跳过及持久连接逐请求关联，覆盖 1xx/HEAD/chunked/升级与不明确边界；不缓存任意正文，也不推进暂停的 P0 全链路。
- 每条 HTTP 观察记录采用单一写入者，避免隧道结束时用旧 Flow 克隆覆盖新观察结果；HTTPS 解密与设备端到端继续单列。

### Android 重启探针就绪条件

修复来源 `23e06877989ef2201205dabfd0f2dc4abf85abe9` 的[桌面 Check](https://github.com/godstealer/http-capture/actions/runs/43671382449) 和 [iOS Mac 验证](https://github.com/godstealer/http-capture/actions/runs/43671382438) 已成功，后者实际完成模拟器启动及无签名 iPhone 编译。Android 双 ABI 编译、ELF/运行库检查和 APK 打包也已通过；[模拟器烟测](https://github.com/godstealer/http-capture/actions/runs/43671382519) 在代理 hint 开关重启后仍发现 `explicit-proxy` 来源，未记为通过。

原探针仅等待 `ConnectivityManager.getDefaultProxy()` 的值。现在在原 5 秒限期内同时等待当前 Probe UID 的 VPN 网络、框架代理状态及针对实际 URI 的进程 `ProxySelector` 选择一致，再使用普通 `URL.openConnection()`。这防止仅凭框架查询值就提前执行“无代理 hint 的 TUN”断言；不设置 `NO_PROXY`、不改系统属性/选择器，也不放宽原生来源必须为 `tun` 的硬断言。Android 将代理配置应用到进程属性及选择器的机制见[官方 Proxy 实现](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/net/Proxy.java)；当前日志支持就绪状态不一致的判断，尚不证明唯一内部传播路径。

纯 JDK 就绪门控 25 项与 API36 Java 编译通过后，`94c1e4d` 的[Android 模拟器烟测](https://github.com/godstealer/http-capture/actions/runs/43674526716) 实际成功：`smokeStatus=ok`、`INSTRUMENTATION_CODE: -1`。普通 openConnection、原 TUN 来源断言与生产来源分类保持；这不替代 PC/手机扫码远程配对的完整设备验收。
