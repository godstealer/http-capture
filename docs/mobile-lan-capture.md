# 局域网协同抓包

目标：PC 显示一次性配对二维码，Android 扫码、授权 VPN 后，将流量送到 PC 抓包列表。默认模式不要求安装抓包 CA；HTTPS 只显示连接与可见域名等元数据，正文解密必须由用户另行安装并信任 PC 的 CA。

证书边界：Android 7 起，面向新系统的应用默认可能不信任用户添加的 CA；Android 11 起，普通应用不能通过 `KeyChain.createInstallIntent()` 替用户安装 CA，只能提供公钥证书供用户到系统设置安装。扫码配对不能改变目标应用的信任策略。[Android 网络安全配置](https://developer.android.com/privacy-and-security/security-config)、[KeyChain 文档](https://developer.android.com/reference/android/security/KeyChain)。

配对二维码只包含协议版本、PC 局域网地址、接收端口、短期配对凭据和 PC 传输身份指纹，不包含 CA 私钥。PC 只在用户打开配对窗口时接受新设备；凭据首次使用后作废，断开设备时撤销会话。手机校验指纹后建立加密通道，VPN 转发通道必须排除在自身 VPN 路由外，避免回环。

实现清单（2026-10-28，开发分支代码已推送；尚未设备验收）：

- [x] 定义 v1 配对 URI，校验私有局域网 IPv4、端口、32 字节随机令牌、PC 身份指纹、版本及五分钟有效期；本地定向测试通过。
- [x] PC 独立 TLS 接收端绑定所选私有 IPv4；每次启动生成临时身份，扫码令牌五分钟有效、只可兑换一次，之后使用一小时会话凭据。会话可撤销，旧连接随之关闭。不会把无认证抓包代理开放成配对入口。
- [x] PC 设置页显示配对二维码、地址、连接状态和断开/关闭操作；接收端将 TCP 连接或首个可见 HTTP 请求记录为 `android-lan` 来源。设备名称在 2026-11-15 未发布增量中已接通；更完整的 HTTP 元数据尚未实现。
- [x] Android 内置扫码与 PC 地址/指纹确认；在固定 SPKI 指纹和 IP SAN 的 TLS 通道中兑换令牌。远程模式仅在配对完成、VPN 获系统授权后启动。
- [ ] TCP/UDP 完整转发：TCP 通过本机 CONNECT 适配器，UDP 通过本机 UDPGW 适配器，均在受保护的 TLS socket 中送到 PC，由 PC 建立上游 socket；同一 UDP 会话复用源端口。远程模式关闭手机直连 UDP。**Android 设备上的 DNS、UDP、QUIC 和长时间会话仍需验证与修复**。五秒心跳失败会停止 VPN，连接不回退到直连。
- [x] 默认不要求抓包 CA；远程 TCP 隧道不作 MITM，PC 只保存可见连接及首个 HTTP 请求元数据。HTTPS 正文和完整 HTTP 会话记录尚不可见；可选 CA 解密模式尚未实现。
- [ ] 验证：Rust 回环测试已覆盖正确/错误 PC 身份、一次性令牌、重放、心跳、TCP/UDP 透传、UDP 源端口复用和撤销；[桌面同源 Check](https://github.com/godstealer/http-capture/actions/runs/40322052848)、[Android 构建](https://github.com/godstealer/http-capture/actions/runs/40276020966)和[iOS 构建](https://github.com/godstealer/http-capture/actions/runs/40276020908)均成功。Android 模拟器与真机的扫码、VPN 回环、HTTP/HTTPS、DNS/UDP/QUIC、切网、断线及停止清理仍需端到端验收。

当前状态：安全配对与 TCP/UDP 转发代码已通过构建和回环测试，但尚无 Android 模拟器或真机端到端证据，不能宣称局域网协同抓包完成，也不发布移动安装包。

## 设备名称增量（2026-11-15，尚未发布）

Android 使用制造商/型号生成显示标签，不读取序列号、IMEI、MAC 或用户账户。标签独立于应用/进程信息，界面明确标为“设备名称（自报）”；来源应用仍显示未识别的远程客户端，导入文件仍提示来源未经本机验证，手动重放不继承设备标签。

二维码与 op1 一次性配对帧保持 v1。兑换会话后另建同样固定 SPKI、IP SAN 的 TLS 连接，发送 `op5 + sessionToken[32] + u16BE(nameUtf8.length) + nameUtf8`。标签为 1–128 字节，禁止控制/双向格式和行段分隔符；PC 接收完整字段后在同一锁内重验会话令牌与到期时间，才写入当前会话。元数据更新不撤销现有隧道；断开/替换会话会清空标签，状态查询隐藏过期标签。

旧 PC 返回不支持或提前关闭元数据连接时，Android 只有另行 op3 验证会话仍有效后才继续无名称配对。建立 TLS、身份校验与 VPN 回环保护失败，以及未知返回码、会话撤销/过期不能当作兼容性降级吞掉。配对阶段的会话校验不要求 VPN 已启动；VPN 已运行时仍保护远程 socket，原 VPN 心跳继续要求保护成功。

本次本机验证：6 项网关 Rust 回归及 1 项来源 JSON 往返实际通过，包含有名 TCP/UDP 持久化、旧帧兼容、错令牌、过期/替换会话不可写、非法长度/UTF-8 和断开清理；JDK17 设备标签规范化/真实帧 79 项检查通过；模拟 socket 对生产客户端上报/心跳方法的 58 项检查通过（该模拟不代替 Android TLS/VPN）。Android 应用及 instrumentation Java 源码在 API36 SDK 下离线编译通过；完整前端回归、TypeScript/Vite 和 core/desktop 含测试源码检查通过。跨平台 CI、Android 模拟器/iOS Mac 与真实设备仍待本批核对；完整 HTTP 会话、HTTPS 解密及移动端正式安装包均未据此完成。
