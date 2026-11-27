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

## Android 远程 VPN 验证门禁（2026-11-20 首轮 CI 待修复）

新增独立的 `android_lan_fixture` 测试工具，只接受真实网卡上的私有 IPv4 和新建的空临时目录；使用生产 `MobileGateway::start`、同一地址的 IP SAN 与固定 SPKI 身份，以及原 v1 一次性配对/一小时会话。受控 HTTP、UDP 和 DNS-wire 源各用独立高端口，不替换普通代理或放宽 loopback 过滤。控制仅走宿主 stdin，不新增未认证网络控制接口；最长七分钟撤销并释放自身任务。现有桌面 1420/1421 及其数据库、代理、系统证书不改动。

模拟器/APK 安装就绪后才生成五分钟配对值；宿主临时目录 0700、凭据文件 0600，经 adb stdin 写入应用私有文件。instrumentation 参数只有私有文件路径，读取后即删除。错误指纹与已用配对值重放必须被生产路径拒绝，不把网络超时当作身份校验通过。测试日志不打印 URI/token/完整异常或 Bundle，不上传私有目录、数据库、bugreport 或原始日志；只发布通过字段白名单构造的摘要。

`vpn-probe` 运行在独立 UID，确认该 UID 的 VPN 网络已生效、无系统 HTTP 代理提示，然后通过普通 Socket/DatagramSocket 发流量。PC 实际保存两份同 TCP 连接 HTTP 请求的独立完成记录，核对精确原始头、重复字段顺序、自报设备名、`android-lan` 来源、远程身份与四槽 `notRecorded`；二进制 UDP 两份原样返回并复用源端口。测试不会由抓包 app 直接调用 op2/4 来冒充 VPN 转发，也不安装抓包 CA。

DNS 分为两项：对生产虚拟 DNS 地址验证 fake-address 答复；对 PC 高端口发送真实 DNS wire 并验证固定回答，后者只证明 UDP 转发。当前生产仍为本地 Virtual DNS，不声称 Android 系统解析器已经由 PC 提供服务，也不拿模拟器内部域名解析替代完整外网 DNS 验收。

PC 在 origin 与网关仍运行时执行撤销；Android 必须由生产心跳/认证拒绝停止服务、JNI 隧道并释放端口，先确认有效会话已清除，再对仍可连接的固定身份网关验证旧会话确被拒绝。PC 再核对配对/设备名清空、没有未完成元数据和测试源计数未变，防止把 origin 自停或连接失败错当撤销成功。`ProtectedRelay` 现有客户端先半关闭边界未在本批修复；正常 HTTP 测试按长度读完响应后关闭，不由其通过推断该边界完成。

同时修复两项真实生产边界：TCP/UDP 目的地先规范化 IPv4-mapped IPv6，再执行回环、unspecified、multicast 和自网关检查；明确 op2/op3/op4 会话拒绝及本地过期只比较清理该会话，迟到的旧失败不会清新配对。普通网络失败或手动停 VPN 保留可重试的有效配对，避免 PC 仍配对但 Android 丢失凭据。

当前本机结果：完整 core 284 项通过、0 失败（原有真实系统凭据测试 1 项跳过）；编排边界 12 项、实际 Java 会话方法级 mock 460 检查和 API36/JDK17 静态编译通过。新增 fixture 测试已编译，但 Windows 返回拒绝访问（OS error 5，never executed），随后该测试 EXE 不存在；没有匹配可读安全事件确认原因，未将其计为通过，不关闭防护/加白名单/换名绕过。它与旧更新器测试限制分别记录。来源 `db3b9fbcac3662d004eb117a1b3de5e3bf05e323` 的[Check #86](https://github.com/godstealer/http-capture/actions/runs/44662072646)已成功；[Android 首轮 #35](https://github.com/godstealer/http-capture/actions/runs/44662072653)的 Linux fixture 测试/构建和 Java/编排门禁通过，但模拟器组合烟测在 `android-vpn-traffic` 阶段报 `SmokeError`，未进入 PC 流量验证。授权后的日志与脱敏摘要一致，未指明 Android 子步骤。[iOS #21](https://github.com/godstealer/http-capture/actions/runs/44662072661)的模拟器启动及无签名 iPhone/Packet Tunnel 验证亦已成功；2026-11-26 [Check #88](https://github.com/godstealer/http-capture/actions/runs/45740755679) 全绿，[Android #36](https://github.com/godstealer/http-capture/actions/runs/45740755677) 的 Linux fixture 测试通过，脱敏摘要确认 `vpn-ready` 后独立 UID 探针在 `probe-http` 失败。两轮探针及 fixture 约定的 HTTP 字节一致。连接、首份响应和第二份响应的故障仍须区分，本地下一批补充固定 HTTP 子步骤与错误分类。当前不能宣称远程端到端已通过。

相机扫码、真机权限/安装、网络切换、长期会话、HTTPS 可选解密与签名 iPhone 仍须分别验收；不发布正式移动安装包。此门禁不会推进暂停的 P0 大正文或 iOS App Store 工作。

## HTTP/1 定界与逐请求关联（2026-11-19 开发增量）

本批在原 TCP 只读观察上增加持久连接逐请求关联，已集中推送开发分支并通过同源验证，尚未纳入正式包。每份请求使用独立序号及 FIFO 响应关联；首份请求复用连接占位记录，后续请求从未被上一请求污染的隧道种子生成新记录。保存原始请求/响应头和字段顺序；正文只按计数跳过，不保存、不解码、不交给正文或重放入口，四个 `bodyCapture` 继续为 `notRecorded`。

支持单个十进制 Content-Length、单一 `Transfer-Encoding: chunked` 的分片计数与 trailer 跳过。没有长度的请求按零正文处理；没有长度的普通响应采用关闭定界，只在服务器干净 EOF 完成，不在其数据中扫描下一份 HTTP。重复 Content-Length、TE/CL 共存、重复或非单一 chunked 的 TE，以及 chunk extensions 暂不观察；这些支持范围以外的情况停止观察而不是拒绝原流量。尺寸计数用有溢出检查的 u64，不缓存正文。

响应优先处理 HEAD、非 101 的 1xx、204/304 与 CONNECT 2xx/101；HEAD 中表示资源长度的 CL 不会让观察器误跳下一响应。205 仍按实际定界处理，不当作 204。每条请求最多跳过 8 个临时响应，累计临时与最终响应头最多 64 KiB，每份最多 256 个字段。CONNECT/Upgrade 设置请求屏障，结果明确前的提前数据不当作下一 HTTP；协议切换后保持透明，后续字节不解析。当前请求正文尚未定界完成就收到最终响应时，仅保存头后保守停止关联，避免早拒绝场景下按旧 CL 错配新请求。

单条隧道最多 8 个未完成请求；队列最多 64 个事件，排队或保存中的原始头字节许可总计 256 KiB（不是进程总内存上限）。chunk size 行最多 8 KiB，每份 chunked 正文的尺寸行、CRLF 与 trailer 元数据累计最多 64 KiB，trailer 最多 256 字段。超限、歧义或事件队列满使观察器全局退出，单一 sender 当场关闭；即使另一方向空闲，也能排空既有事件并将未完成记录标为中断，继续转发原始字节。

单一写入者仅保留尚未完成的记录。只有请求和响应的定界均已完成才保存 `completed` 并移除；这表示观察到 HTTP 边界，不表示正文已记录。已完成记录不会在隧道结束时被旧快照覆盖，后续网络失败只影响仍未完成的请求。真实 IO 失败保留转发错误，解析或观察预算停止显示独立的元数据中断。删除/清空后仍只更新存活行，不重试创建旧 ID；后续实际新请求可继续产生新记录。记录时间使用读段事件时间，避免用数据库排队结束时间替代。

本机完整 core 282 项通过、0 失败（原有真实系统凭据测试 1 项跳过），MCP 43 项、控制 HTTP API 19 项及桌面 browser-replay 含测试源码检查通过。新增 20 项定界/FIFO 单测、8 项真实认证 TLS 或双向流集成、2 项事件/字节预算回归均实际通过；包含受控 AsyncRead ConnectionReset、首条完成快照不变、真实 SQLite 更新失败后持续转发、删除/清空不复活与严格半关闭。独立审查未发现阻断；未形成完整头的半份下一请求仍不补造记录，本批不声称提供该截断场景的完整诊断。来源 `f6a637f22fae93d192000f529ee578f543cfe960` 的[Check #85](https://github.com/godstealer/http-capture/actions/runs/44444363001)、[Android 构建与模拟器 #34](https://github.com/godstealer/http-capture/actions/runs/44444363004)和[iOS Mac 工作流 #20](https://github.com/godstealer/http-capture/actions/runs/44444363048)均已成功，公开状态与提交身份已核对。用户运行中的 1420 前端与 1421 内核不重启或替换；扫码/VPN 远程设备端到端、长期切网、HTTPS 可选解密和签名设备仍未验收，不发布正式移动安装包。Windows 提前 TLS shutdown 的既有 10054 边界仍未声称修复，严格双向流半关闭测试不替代该 TLS 场景。

## 首份 HTTP 头观察（2026-11-18 开发增量）

认证 TCP 隧道现使用只读观察器，识别首份完整 HTTP/1.0 或 HTTP/1.1 请求头，以及其后的最终响应头。请求方法、可用 URL、客户端协议、状态、字段顺序、重复字段、大小写、Latin-1 字段值与原始头字节可供已有详情界面查看；原始 `Host` 只被记录，实际转发仍使用认证隧道目的地。origin-form URL 由该目的地拼接并校验，无法表达为有效 HTTP URL 时保留 `tcp://` 连接地址及原始头。

每方向累计最多 64 KiB 头字节、每份最多 256 个字段；最多跳过 8 份非 101 的 1xx，累计字节预算不会随临时响应重置。101 作为本批最后一份响应，升级数据不继续解析。TLS、HTTP/2 前言、畸形或超限头停止观察；请求尚未被完整识别前出现的服务器数据使该方向永久退出响应识别，避免把后续正文误作响应头。慢请求与分片按实际读段增量处理，取消原先 200 ms 首读采样，不提前消费网络字节。

观察器不保存正文，`Content-Length: 0` 也不等于完成正文采集，四个 `bodyCapture` 状态继续为 `notRecorded`。该阶段尚未接入后续请求和响应、正文 framing 与 FIFO；下一批本地进展见上文，HTTPS 解密仍未接入。本批不会使重放或正文下载入口绕过既有守卫。

转发保持 `copy_bidirectional` 的字节、背压和半关闭行为；解析失败、观察队列关闭/满及持久化失败只停止记录。每隧道至多两条头事件，独立写入者串行保存初始连接、请求头、响应头和最终生命周期，先排空头事件再处理结束结果，避免旧快照覆盖。写入者并发有界；没有观察预算时连接仍转发。所有观察写入在单个数据库事务内检查行存在性和删除标记；清空/删除已存在记录后不会被结束事件恢复，也不发布虚假成功通知。撤销与过期同时约束目的地读入、DNS/连接、确认和空闲转发；结束时先关闭网络句柄，再等待记录保存。等待写入者结束期间仍占网关连接配额；极慢或卡住的数据库可能延后接纳新连接，本批不声称记录延迟与服务容量完全独立。

本机验证：完整 core 库 252 项通过、0 失败，原有使用真实系统凭据库的测试 1 项仍跳过；MCP 43 项通过。新增观察相关 25 项覆盖全部分片位置/逐字节、原始头、1xx/101/总预算、非 HTTP/H2/TLS、慢发、双向完整字节、真实 TLS 正常 HTTP 流程、泛型流半关闭、空闲到期/撤销、取消、清空/删除与真实 SQLite 初始/后续写入失败。控制服务完整 19 项真实 HTTP/API 回归通过，桌面 browser-replay 含测试源码检查通过；来源 `92bb0ed3b39f018a765979c0deb4f423155ee046` 的[Check #84](https://github.com/godstealer/http-capture/actions/runs/44222514649)、[Android 构建与模拟器 #33](https://github.com/godstealer/http-capture/actions/runs/44222514601)和[iOS Mac 工作流 #19](https://github.com/godstealer/http-capture/actions/runs/44222514875)均已成功，公开状态与提交身份已核对。

已知测试环境边界：Windows 上客户端先 TLS `shutdown` 再等待上游响应会出现 10054；无观察器的原始 `copy_bidirectional` 基线同样复现，因此本批未将它声称为已修复。真实 TLS 测试使用按请求长度分帧、响应完成后再关闭客户端的正常 HTTP 顺序；另用严格的双向流测试验证客户端先半关闭后仍收全响应，带/不带记录均检查完整字节和成功统计，不吞网络错误。

本批仍为未发布开发增量；用户正在使用的前端与内核保持原进程，不替换其运行数据库、规则、系统代理或证书。完整扫码/VPN 远程端到端、Android/iPhone 设备签名与长期网络验收仍待完成，不发布正式移动安装包。

## 正文可用性契约（2026-11-16 开发增量）

正文是否记录与正文长度、请求是否完成分别表示。`bodyCapture` 可为 `request`、`response`、`originalRequest`、`originalResponse` 指定 `recorded` 或 `notRecorded`；`recorded` 仍需结合既有生命周期判断是否接收完毕。旧普通抓包缺少该字段时保留原语义；旧 `android-lan` 缺字段时一律保守视为未记录，显式 `recorded` 才能覆盖此回退，真实空正文仍允许读取为零字节。

新 PC 网关明确标记元数据记录；Store 的当前投影、旧 0 B 缓存、完整 Flow 与 MCP 都采用同一判定，不把占位字符串或过期正文引用当成完整实体。连接/可见头部仍可查看和搜索；对应正文的读取、下载、复制编辑/重放、cURL、HAR 与 MCP 组合请求在未记录时拒绝。请求与响应状态独立，已记录的响应不因请求未记录而失去只读能力。Android/iOS 的正文摘要、查看与复制入口同样检查状态。

会话 JSON 保留完整元数据与显式状态，未记录实体的兼容 `bodyBase64` 为空占位而不是长度证明。只要实际存在的实体中含 `notRecorded`，文件封装采用 `http-capture-session` version 2，让旧版读取器明确拒绝；全部实体已记录时继续使用 version 1。新导入器同时接受 1/2，并对旧 LAN 快照归一化。共享核心在同一只读事务内先扫描最小元数据选择版本，再逐条写出，版本与内容不会跨快照；不读取全部历史正文做预扫，也不推进暂停的 P0 大正文链路。

本批仅为开发增量，尚未发布新桌面或移动安装包。模型/Store 契约 12 项、受影响 Store 54 项（包含前述 Store 用例）、认证网关 6 项、MCP 43 项均在本机实际运行通过；Android 纯 Java 回归与 API36 离线编译通过。完整前端回归、类型检查、生产构建、4 项真实 HTTP 控制 API 回归及桌面含测试源码检查均已通过。来源 `e8930ee9f7537e5109d825e7d9e2720ad139dd07` 的[Check #83](https://github.com/godstealer/http-capture/actions/runs/43793782841)、[Android 构建与模拟器](https://github.com/godstealer/http-capture/actions/runs/43793782823)及[iOS Mac 工作流](https://github.com/godstealer/http-capture/actions/runs/43793782877)均已成功，公开运行状态与提交身份已核对。iOS 的同源 Mac 工作流已通过；这不替代签名 iPhone、Android 真机或扫码/VPN 远程端到端验收。

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

- 正文可用性契约及首份头观察的同源 Check、Android/iOS 已通过；本批已实现有界正文计数跳过和持久连接 FIFO 元数据关联，本机检查及同源 Check、Android/iOS 均已通过，尚未进入安装包。
- 后续优先补齐 Android 模拟器连接生产 PC 网关的远程配对、VPN TCP/DNS/UDP 转发证据；真实扫码摄像头、真机权限与长期切网仍须分别验收，不能由普通本机 VPN 烟测替代。
- HTTP/1 目前只支持明确的单 CL 与 bare chunked 子集；chunk extensions、组合传输编码及其他未支持定界均透明转发并停止观察，后续依据实际流量需求扩展，不猜测边界或缓存任意正文。
- 保持单一观察写入者、逐请求 ID、原始字节/背压/半关闭和 `notRecorded` 守卫；HTTPS 可选解密与设备正式包继续单列，P0 大正文全链路仍暂停。

### Android 重启探针就绪条件

修复来源 `23e06877989ef2201205dabfd0f2dc4abf85abe9` 的[桌面 Check](https://github.com/godstealer/http-capture/actions/runs/43671382449) 和 [iOS Mac 验证](https://github.com/godstealer/http-capture/actions/runs/43671382438) 已成功，后者实际完成模拟器启动及无签名 iPhone 编译。Android 双 ABI 编译、ELF/运行库检查和 APK 打包也已通过；[模拟器烟测](https://github.com/godstealer/http-capture/actions/runs/43671382519) 在代理 hint 开关重启后仍发现 `explicit-proxy` 来源，未记为通过。

原探针仅等待 `ConnectivityManager.getDefaultProxy()` 的值。现在在原 5 秒限期内同时等待当前 Probe UID 的 VPN 网络、框架代理状态及针对实际 URI 的进程 `ProxySelector` 选择一致，再使用普通 `URL.openConnection()`。这防止仅凭框架查询值就提前执行“无代理 hint 的 TUN”断言；不设置 `NO_PROXY`、不改系统属性/选择器，也不放宽原生来源必须为 `tun` 的硬断言。Android 将代理配置应用到进程属性及选择器的机制见[官方 Proxy 实现](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/net/Proxy.java)；当前日志支持就绪状态不一致的判断，尚不证明唯一内部传播路径。

纯 JDK 就绪门控 25 项与 API36 Java 编译通过后，`94c1e4d` 的[Android 模拟器烟测](https://github.com/godstealer/http-capture/actions/runs/43674526716) 实际成功：`smokeStatus=ok`、`INSTRUMENTATION_CODE: -1`。普通 openConnection、原 TUN 来源断言与生产来源分类保持；这不替代 PC/手机扫码远程配对的完整设备验收。
