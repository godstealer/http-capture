# Android / iOS 移动端

状态：Android 已重新构建包含 TUN 透明代理桥的 arm64-v8a / x86_64 双架构调试 APK，并核对包内库、JNI 入口和签名；尚未经过真机验收。iOS 宿主、Packet Tunnel 与 Rust 包桥接已实现基础链路；此前版本曾通过 Simulator 与无签名 iPhone arm64 编译，近期 App Group、宿主 Rust 链接和离线功能改动尚未经过 Mac 重编。签名及 iPhone 设备验收尚未完成。当前没有可交付的正式移动安装包。

iOS 请求列表现按已有来源字段标注抓包或手动重放，并在无记录或 URL 搜索无结果时显示提示。Swift 界面改动尚待下次 Mac 构建和模拟器/设备验收。

从抓包记录复制为新请求的正文上限已与手动请求编辑器对齐为 1 MiB：Android 和 iOS 均逐段读取原始字节；可读文本超过 64 KiB 时自动改用 Base64 编辑，避免文本输入上限阻止重发。超过 1 MiB 的记录仍拒绝复制。Android 字节级回归与调试 APK 构建通过，最新 APK 的 v2 签名验证通过；iOS 源码和回归用例已更新，需下一次 Mac CI 编译并在设备上验证复制、编辑及发送。

2026-10-25 本地新增抓包记录导出：Android 通过系统文件选择器保存 `http-capture-session` v1 JSON；iOS 在 VPN 扩展内生成同格式文件，以 32 KiB 分块传给宿主，再通过系统分享菜单保存。共享存储层使用独立 SQLite 读取快照，逐条恢复正文并写出，避免一次载入全部历史。单条超大正文仍会完整进入内存；此前暂缓的超大正文低内存链路尚未解决。共享存储测试、Android 双架构 APK 与仪器测试源码编译、iOS Rust 桥接测试已通过；Android 文件选择器和 iOS Swift 界面仍需设备或 Mac 构建验收。导出包含请求与响应正文，分享前请确认目标位置。

并发回归已在导出写出第一条记录时插入一条新抓包记录，确认导出的会话只包含读取快照时的两条记录，数据库则保留三条；`cargo test -p capture-core session_export` 两项通过。另将导出文件交给桌面端同用的导入存储接口，确认两条记录与外置大正文均能恢复。桌面前端导入 Worker 目前限制单条序列化记录不超过 60 MiB，并会读取整个会话文件；因此“桌面格式兼容”不代表超大文件可低内存导入。上述本地测试不替代设备文件选择器验收。

iOS 导出界面显示按字节计算的进度，可在分块传输中取消；取消与失败会关闭并删除宿主临时文件，扩展侧通过导出令牌释放文件。取消发生在扩展生成文件期间时，界面会等待该操作返回并清理旧令牌后才允许重新导出；VPN 断开时会废弃宿主暂存与迟到回调。开始下一次导出时会删除上一次尚未分享出去的临时副本。此 Swift 增量尚未经过 Mac 编译或设备验收。

2026-10-25 最新本地 Android 调试 APK：`apps/mobile/android/app/build/outputs/apk/debug/app-debug.apk`，121,486,899 字节，SHA-256 `39e7421bdee60baef6472b6465ed377607e3e6348b2e8d81a444bdb1f0d2b79a`。`apksigner verify` 确认 v2 签名有效，包内有 arm64-v8a 与 x86_64 两份原生库，manifest 含 VPN 服务及前台服务、通知权限。此包尚未安装到设备，本地包也未作为正式发行附件上传。

Android 的历史会话 JSON 现在可在停止 VPN 和本机代理后导出。运行时复用当前存储，停止后按应用私有目录打开 `sessions.db`，将记录写入同一种桌面可导入格式；首次尚无记录时会导出空会话。Rust 定向测试验证关闭存储后仍能导出已保存的抓包记录；Java 与 instrumentation 源码离线编译、双架构 APK 重建及 v2 签名验证通过。设备上的系统文件选择器、停止后导出以及大文件复制仍待验收。

Android 停止抓包后现在还能搜索、翻页、打开历史详情并按段查看请求与响应正文；重启应用后也会自动加载本地记录。离线读取在应用进程内复用同一个存储实例，使详情给出的正文修订令牌在后续分段读取时仍有效；若记录确实变化，原有版本校验继续生效。Rust 测试覆盖停抓后的列表、详情、正文和会话导出，Android 应用与 instrumentation 源码已编译，双架构 APK 已重建；设备界面交互仍待验收。

Android 请求库也可在停抓状态下打开：新建或从抓包记录复制的请求可先编辑、保存到集合，已有请求可搜索、修改、移动和删除，集合可重命名或删除空集合；无需启动 VPN 即可发送已保存请求。Rust 测试覆盖离线保存、检索、删除以及运行状态切换；设备测试增加停抓后再次执行请求库操作，Java 与双架构 APK 离线编译通过，触屏交互仍待设备验收。

Android 阻止规则和响应修改规则现在也可在停抓状态下查看、编辑和保存；启动抓包后会读取离线保存的配置，运行时编辑仍立即更新当前内核。响应修改继续使用配置版本号拒绝旧页面覆盖新配置。Rust 测试覆盖离线保存、启动后读取、运行时更新及版本冲突；Java 应用与设备测试源码已编译，双架构 APK 已重建，实际触屏与流量效果仍待设备验收。

Android 停抓时现在也能先生成并保存 CA 公钥证书，再到系统设置中安装；之后启动代理使用同一张证书。上游 CA 证书可在停抓时导入、启用或删除，版本号仍防止旧页面覆盖新配置。Rust 测试覆盖首次离线 CA 生成、仅导出公钥、离线信任证书配置、启动后 CA 与证书列表一致；设备测试源码增加停抓后导出的 CA 一致性和离线证书管理检查。双架构 APK 与设备测试 Java 源码已编译，文件选择器、系统证书安装和手机 HTTPS 流量仍待设备验收。

Android 手动 HTTP(S) 请求现在不需要先启动 VPN 或本机代理：停抓时可直接发送，内核用同一套重放逻辑创建请求记录、收取响应并写入历史；抓包运行时继续复用活动内核。Rust 回环服务器测试确认离线发送返回 HTTP 200 且历史记录可检索，设备测试源码也加入停抓后的 JNI 发送与持久化检查。双架构 APK 已重建，实际手机网络、编辑器触屏和规则应用仍待设备验收。

iOS 宿主与 Packet Tunnel 现使用同一个 App Group 保存抓包数据库和证书。扩展首次启动时会将旧私有目录复制到共享目录，保留原件；VPN 停止后，宿主通过只读 Rust 接口搜索、翻页、打开详情并分段查看正文，运行时仍通过扩展消息读取。Rust 离线历史测试 10 项通过；这批 Swift/App Group 改动尚未经过 Mac 编译、签名或 iPhone 验收。签名配置还需为宿主和扩展启用 `group.dev.httpcapture.mobile.ios`。

iOS 停止 VPN 后也可从共享数据库导出完整抓包记录 JSON；宿主直接写入应用临时文件供系统分享，不需要将大文件分块传过扩展消息。Rust 测试核对导出包含请求 ID 和正文，并确认已有目标文件不会被覆盖。导出期间允许取消，取消后的结果文件会清理；这一轮 Swift 界面仍待 Mac 编译与设备验收。

iOS 停抓时现在可打开请求库、按集合与关键词搜索、读取分段正文、保存或删除请求，并重命名或删除空集合；从历史记录复制出的请求也可在停抓状态编辑和保存。离线保存由宿主直接写入 App Group 数据库，最多支持 1 MiB 正文。为了避免升级时先创建空共享库而跳过扩展旧数据迁移，首次使用请求库需要先启动一次 VPN。Rust 桥接测试覆盖 160 KiB 原始正文的离线保存、分段还原、集合管理和删除；Swift/Xcode 编译及 iPhone 操作尚待验证。

iOS 手动 HTTP(S) 请求也可在停抓时发送：宿主用同一 `Engine::replay` 路径直连上游，将请求和响应存入 App Group 历史；运行时仍由 Packet Tunnel 处理。离线发送直接通过宿主 Rust 桥接传入最多 1 MiB 正文，无须受扩展 128 KiB 消息限制。首次使用仍需启动一次 VPN 完成共享目录迁移。本机回环服务器测试验证停抓时收到请求、返回 200 并写入可检索历史；Swift/Xcode 与 iPhone 网络验收尚待完成。

iOS 停抓时还可查看和编辑请求阻止规则、整份响应替换与部分内容修改规则。宿主复用共享内核的配置校验和 App Group 数据库，响应规则更新继续检查配置版本，避免旧页面覆盖新版本；再次启动 VPN 时会载入离线保存的规则。Rust 测试覆盖离线保存、重新打开内核后的持久化和旧版本拒绝；Swift/Xcode 编译、界面交互及实际流量效果仍待验证。

本机离线手动发送集成测试现进一步验证规则的实际效果：HTTP 上游返回 200 和 `real`，保存的响应替换规则让调用方收到 201 和 `ok`；另一条域名阻止规则让未连接目标主机的请求直接得到 403。iOS Rust 桥接 15 项测试通过。这证明宿主离线重放链路会应用规则，但仍不能代替 iPhone 系统 VPN 抓包流量的验收。

iOS 停抓后也可导出抓包 CA 公钥证书，并导入、启用、停用或删除上游 CA。宿主使用共享目录中的证书库，保持配置版本校验；CA 分享文件仅含公开证书，不含私钥。首次仍须启动一次 VPN 完成旧目录迁移。Rust 桥接测试验证公开 CA、离线导入与启用、旧版本拒绝及重新打开内核后的配置；14 项测试通过。Swift/Mac 编译、系统证书安装和真实私有 CA 站点仍待设备验收。

2026-10-24 首次远端构建：同一提交的[桌面 Check](https://github.com/godstealer/http-capture/actions/runs/40053342564)前端、Linux、macOS 和 Windows 全部通过；[Android 构建](https://github.com/godstealer/http-capture/actions/runs/40053342578)在 SDK 安装阶段因 action 默认请求已移除的 `tools` 包而失败，未编译 APK；[iOS 构建](https://github.com/godstealer/http-capture/actions/runs/40053342628)通过 Swift 测试、Xcode 工程生成和无签名模拟器 App/Packet Tunnel 编译，随后因 `lipo` 校验命令参数顺序错误而中止，尚未编译 iPhone 目标或上传附件。两处工作流脚本随后一并修复并完成重跑。

第二批提交 `d9fca00` 的[桌面 Check](https://github.com/godstealer/http-capture/actions/runs/40055538553)四个平台全部通过；[Android mobile build](https://github.com/godstealer/http-capture/actions/runs/40055538618)成功编译双架构 APK 并上传 `http-capture-android-debug` 附件；[iOS mobile build](https://github.com/godstealer/http-capture/actions/runs/40055538644)成功编译并上传 `http-capture-ios-simulator` 附件，随后无签名 iPhone arm64 宿主与 Packet Tunnel 编译及架构校验也通过。构建结果不能替代 Android/iPhone 真机 VPN、HTTPS 证书信任、网络切换和界面操作验收；iOS 设备安装仍需签名与 Packet Tunnel 权限。

已单独下载 Android CI 附件并检查 APK：大小 108,143,866 字节，SHA-256 为 `b2dfda0858935fd66192588990ffd3be122e748771ea4d3abf6474fbfcb29131`，包内同时有 arm64-v8a 和 x86_64 的 `libhttp_capture_android.so`。APK 含 Android 签名块；此项检查未代替 `apksigner` 或真机安装验证。

Android 新增模拟器冒烟测试：在 API 33 x86_64 模拟器安装构建的 APK 和独立测试 APK，在应用进程内经 JNI 启动回环代理，让本机 HTTP 服务返回固定内容，再检查响应、持久化请求记录和导出的 CA 公钥证书，最后停止代理并启动主界面。[Android mobile build](https://github.com/godstealer/http-capture/actions/runs/40058540465) 的模拟器测试已通过（`smokeStatus=ok`、`INSTRUMENTATION_CODE: -1`、主界面启动成功）。后续[Android mobile build](https://github.com/godstealer/http-capture/actions/runs/40062698192) 已通过系统 VPN 授权、隧道建立、状态读取、停止清理和主界面启动测试（`smokeStatus=ok`、`INSTRUMENTATION_CODE: -1`）；对应[桌面 Check](https://github.com/godstealer/http-capture/actions/runs/40062698108) 四项全绿。[独立 App 原始 TCP 流量测试](https://github.com/godstealer/http-capture/actions/runs/40063791577) 也已通过：另一 UID 的测试 App 经 VPN 向模拟器宿主机发送 HTTP 请求，宿主机收到 `/vpn-smoke`，抓包数据库出现对应记录，随后 VPN 停止及主界面启动成功；对应[桌面 Check](https://github.com/godstealer/http-capture/actions/runs/40063791572) 四项全绿。模拟器结果不能替代真机 HTTPS、证书信任、切网、IPv6、UDP/QUIC 和长期运行验收。

下一批 Android 模拟器测试已增加 HTTPS 拦截检查：回环代理测试以导出的 CA 公钥建立只信任该 CA 的 TLS 客户端，通过 CONNECT 发起请求并启用仅匹配该路径的阻止规则，检查 TLS 主机名验证、解密后的 HTTP 403 响应和持久化记录。独立 UID 的测试 App 也会经 VPN 向模拟器宿主地址发起 TLS 请求，以同一 CA 验证证书并检查解密后的规则命中记录。[Android mobile build](https://github.com/godstealer/http-capture/actions/runs/40081818103) 成功：双架构 APK 和测试 APK 均构建通过，模拟器 instrumentation 返回 `smokeStatus=ok`、`INSTRUMENTATION_CODE: -1`，原始 VPN HTTP 到达宿主服务，主界面启动成功。这两条 TLS 测试均不依赖公网服务；它们仍未证明真实上游 TLS 握手成功或手机系统证书安装。

## Android 当前代码

2026-10-25：手动新建请求增加“保存请求”和“已保存请求”入口。保存时把已组装的有序请求头、停用状态和最终正文写入共享请求库的“我的请求”集合；列表每页最多读取 100 条，选中后可重新打开、修改、覆盖保存或发送。若库中请求包含当前移动编辑器无法无损表达的 gRPC、HTTP/2 伪头、上游代理、启用脚本或自定义 TLS 配置，会明确拒绝打开。Android arm64-v8a / x86_64 调试 APK 和模拟器测试源码已在本地编译；模拟器保存/读取测试及设备界面验收仍待运行。该批改动尚未推送或发布。

2026-10-25：Android 与 iOS 的已保存请求列表增加名称、方法、URL 的不区分大小写搜索，最多输入 256 字节；筛选后再分页，页数和总数与当前搜索结果一致。Rust 双端桥接的离线交叉编译检查、Android 双架构原生库与调试 APK 重编、Android 测试 APK 编译通过；实际设备交互和 Swift 编译仍待验证。

2026-10-25：Android 保存请求时可选择已有集合或在同次保存中创建集合；编辑旧请求默认保留原集合，也能显式移到另一集合。已保存列表显示集合名称。原生桥接校验集合是否存在及新名称是否重复，测试源码覆盖创建、迁移与拒绝无效集合；Rust 交叉编译与 Android 测试 APK 编译通过，设备操作仍待验收。

2026-10-25：Android 请求库增加“全部集合 / 单个集合”切换，集合筛选可与名称、方法、URL 搜索叠加；列表总数与上一页、下一页按当前组合条件计算。仪器测试源码覆盖集合筛选及请求移走后的空集合，实际设备操作待验收。

2026-10-25：Android 在单个集合视图提供“管理当前集合”，可重命名或删除空集合。删除前再次由原生存储检查集合内是否仍有请求，非空时拒绝删除，不会级联删除请求；重命名失败时保留输入供修正。仪器测试源码覆盖非空拒绝、空集合重命名与删除；Rust 交叉编译、Android 测试 APK 编译和双架构调试 APK 重建通过，设备界面和运行结果待验收。

2026-10-25：iOS 请求库增加集合列表、按集合筛选、保存时选择或新建集合，以及重命名和删除空集合。直接保存与分段正文上传共用集合选择；编辑旧请求时默认保留其集合。Rust 扩展命令测试覆盖创建、迁移、筛选、非空删除拒绝、空集合删除，以及 1 MiB 分段正文保存到新集合并在编辑后保留集合；Windows 本机 9 项测试和交叉目标源码检查通过。Swift 测试增加编辑草稿保留集合 ID 的断言；Swift/Xcode 编译、模拟器和 iPhone 交互仍待 Mac 环境集中验证，本批改动尚未推送或发布。

iOS 请求库在列表读取期间完成保存或删除时，会排队再读一次列表和集合数量，避免较早的读结果把刚写入的状态覆盖；VPN 停止时清除排队刷新。此 Swift 控制器改动仍待 Mac 编译和界面验收。Android 和 iOS 的抓包记录 JSON 导出已接入，见文首；Android 文件选择器与 iOS 分享菜单仍待设备验收。

Android 打开已保存请求时，会将每个请求头转成编辑器文本并重新解析，确认名称、值、顺序和停用状态未改变；带首尾空格等无法无损表示的字段直接拒绝编辑。方法或 URL 若会在保存时被规范化改写，请求头超过 16 KiB，或使用非默认脚本模块，也会拒绝打开，避免覆盖保存时丢失配置。此保护及模拟器回归源码的离线测试 APK 编译已通过，模拟器实际运行尚待集中 CI。

请求库列表增加长按删除：用户确认后只移除所选请求，并重新打开有效页。Android 双架构原生库已离线重编，新 JNI 符号在 arm64-v8a 与 x86_64 中均可见；调试 APK 和测试 APK 已重新组装。模拟器实际删除回归和触屏操作尚待运行。

Android VPN 的 TCP 来源识别已接入：`tun2proxy` 在连接本机透明代理时记录原始 TUN 源/目标地址与回环连接端口；VPN 服务在 Android 10（API 29）及以上用 `ConnectivityManager.getConnectionOwnerUid` 查询原始连接的 UID，再尝试取得包名和应用名。共享内核按回环端口一次性关联抓包记录，无法匹配或系统拒绝查询时保持未知，不把转发器误标成来源应用。Android 抓包列表直接显示已识别的应用名，应用名缺失或为空时显示 UID；存储层仅投影有界的来源字段，不读取正文。Android 11 起包可见性可能使应用名不可用，此时仍可显示 UID；Android 9 及以下暂不支持 UID 查询。核心集成测试已验证来源写入、端口隔离和旧记录兼容；[Android mobile build](https://github.com/godstealer/http-capture/actions/runs/40088853655) 通过双架构 APK 构建与模拟器独立 App 的 UID 存在性断言（`smokeStatus=ok`、`INSTRUMENTATION_CODE: -1`）。已进一步加入“详情与列表摘要的 UID 均等于测试 App 自报 UID”的断言，测试 APK 编译通过，待下批 CI；真机应用名显示仍待验证。实现依据为 Android 官方的 [getConnectionOwnerUid](https://developer.android.com/reference/android/net/ConnectivityManager#getConnectionOwnerUid(int,%20java.net.InetSocketAddress,%20java.net.InetSocketAddress)) 文档。iOS 当前 Packet Tunnel 只有原始 IP 包，不能沿用 Android UID 查询；Apple 的 [NEFlowMetaData](https://developer.apple.com/documentation/networkextension/neflowmetadata) 仅在按 App 配置的 VPN 中包含来源信息，现有全局 Packet Tunnel 先保持 TUN 来源未知。

- `apps/mobile/android/native` 是独立 Rust `cdylib`，通过 JNI 打开 `capture-core`、启动手机本机回环 HTTP(S) 代理、读取不含正文的流量元数据以及导出 CA **公钥证书**。CA 私钥仍留在应用私有数据目录。
- `apps/mobile/android/app` 是原生 Android 工程，可输入并保存端口、选择需要抓包的应用（桌面应用可勾选，后台应用可按包名添加；VPN 启动时会校验应用是否仍已安装）、启动/停止抓包、按 URL 包含搜索、按每页最多 100 条浏览全部历史摘要、点开请求元数据并按每段 16 KiB 浏览请求/响应正文，并用系统文件选择器保存 CA 证书。首页实时刷新；搜索与旧页均使用数据库游标按需读取，页面不会一次加载全量历史。可手动新建 HTTP(S) 请求，输入方法、URL 和有序文本请求头，发送后写入现有重放记录并打开详情；正文支持文本、JSON、URL 表单、Base64 原始字节、系统文件选择及简单 Multipart 字段/单文件上传；文本最多 64 KiB，文件与最终请求正文最多 1 MiB，完整高级请求编辑器仍未实现。阻止规则页可添加精确域名、URL 包含或 URL 正则规则及可选请求方法，也可启停全局或单条规则、删除规则，复用内核校验与持久化。前台 `VpnService` 负责授权、建立 IPv4/IPv6 虚拟网卡和服务生命周期；JNI 通过 `tun2proxy` 将 TCP 与虚拟 DNS 导向本机 `capture-core` 透明代理入口。该入口识别 HTTP/TLS；非 TLS、非 HTTP 的 TCP 直接转发，包含服务端先发数据的协议，不写入 HTTP 抓包记录。TLS ClientHello 仅声明非 HTTP ALPN 时，或命中解密绕过列表时，原样转发握手；未声明 ALPN 的非 HTTP TLS 仍需专门识别和设备验收。VPN DNS 服务地址为 `10.111.0.1`，与虚拟域名地址池分开。虚拟 DNS 以独立的 IPv4/IPv6 地址池响应 A/AAAA，其他查询类型返回空答案，双栈虚拟地址均可反查原始域名。对非 DNS UDP，使用本地维护的 `tun2proxy` 小补丁直连目标，避免 HTTP 代理不支持 UDP 时阻断连接；默认情况下 UDP 内容不进入抓包记录；开启 HTTP/3 抓取后，UDP/443 可进入 QUIC/H3 入口。摘要和正文预览均走有界存储读取，详情仅在点开时载入。

Android 前台通知现提供“停止抓包”操作，点击后走与首页停止按钮相同的 VPN、TUN 和代理清理路径。Android 13 起首次启动 VPN 时会申请一次通知权限；拒绝后抓包仍能运行，但前台服务通知及其按钮不会出现在通知栏，可从首页或系统活动应用停止。依据：[Android 官方通知权限说明](https://developer.android.com/develop/ui/views/notifications/notification-permission)。通知权限与按钮的实际操作仍待设备验收。

模拟器冒烟测试已接入通知停止操作：在安装后授予通知权限，从当前前台服务通知找到“停止抓包”并触发，等待服务清理完成。Android 测试 APK 本地离线编译通过；该场景尚未在模拟器运行。
- Android arm64-v8a 与 x86_64 Rust 原生库已在 Windows NDK 交叉编译并重新打入调试 APK，包含 TUN 透明代理桥。Windows 构建为 OpenSSL 启用 `no-asm`，避开 MSYS Perl 与原生 Make 的 ARM 汇编命令引号冲突；Linux NDK 仍可使用 `scripts/build-android.sh`。已确认 APK 内两架构库与本地构建结果字节一致、各有 13 个 JNI 入口，且调试 APK 签名有效；尚未在设备运行。开发分支移动端文件更新时触发的 `mobile-android.yml` 作为独立 Linux 构建入口，已首次执行，见上文 CI 记录；修复后的完整重跑已通过。虚拟 DNS 的查询类型与双地址池回归 3 项通过。停止 VPN 后界面等待服务清理完代理和 TUN 句柄再允许重启；界面重建时会查询仍在运行的独立本机代理。服务监听非 VPN 的联网网络变化并向 Android 更新底层网络列表，退出时注销回调；现有连接在切网时是否恢复仍需设备测试。详情页已按内核实际的顶层 `FlowMetadata` 结构修正，手动请求的文本输入限制为 64 KiB，请求头最多 16 KiB；文件与最终正文最多 1 MiB。Gradle `:app:assembleDebug --offline` 于 2026-10-24 对 Java/资源部分通过。APK 启动、VPN 流量和真机行为均未验证。本机 `adb devices` 无设备；旧模拟器及新下载的官方 stable 37.2.12 模拟器的 x86_64 后端均在 Windows 加载阶段以 `-1073741515`（`0xC0000135`）退出。新版本能识别 AVD，失败发生在 Android 系统启动前；静态依赖检查没有定位到缺失的具体 DLL，不能据此归因于 APK。IPv6 路由已写入代码但未做设备验证；HTTP/3 仍待手机实测，完整的移动端规则/请求编辑界面尚未实现。

在当前已安装 Android 工具链的 Windows 工作区可运行 `scripts/build-android.ps1 debug both`；Linux 安装 Android SDK、NDK、Rust、JDK 17 和 Gradle 后运行 `bash scripts/build-android.sh debug both`。2026-10-24 本机构建的调试安装包为 `apps/mobile/android/app/build/outputs/apk/debug/app-debug.apk`，SHA-256 为 `404A2F3E71DB5A9A9C0AF685C243A7B21A8AB9E4A25EE779954DE135F09ABD07`。包内 arm64-v8a 与 x86_64 原生库已随共享内核规则更新重新交叉编译，Java/资源也已重编；调试签名校验通过。仓库的 **Android mobile build** 工作流在开发分支的移动端文件更新时自动触发，合入默认分支后也可手动触发，生成 `http-capture-android-debug` 构建附件；它不属于桌面版 Check 或正式发布流程。安装包仍需设备验收 VPN 和证书链路。

共享 TUN 转发层已修复会话计数依赖 `trace` 日志求值的问题：在普通日志级别也会更新活跃会话数；单条 TCP、DNS-over-TCP 或 UDP gateway 连接初始化失败不会结束整个 VPN 主循环，失败的 UDP 会话也会释放名额。虚拟 DNS 3 项与 TUN 回归 4 项通过：模拟 TUN 的普通 IPv4、IPv6 UDP 均可往返本机服务且内容一致，TCP 完成握手、向本机 HTTP 代理发出 CONNECT，并双向传输数据。关闭详细日志后的会话计数回归连续运行 20 次通过，TCP 往返回归连续运行 10 次通过。真实 Android 的 TUN fd、断网、切网和高并发行为仍待设备验证。
Android JNI 在发现隧道任务异常退出后，会等待回环透明代理桥释放再上报错误，避免快速重启时遗留尚在清理的监听器；双架构交叉编译与 APK 组装已通过，异常退出后的真机重启仍待验证。

Android 和 iOS 新增可选的 HTTP/3 抓取开关，默认关闭。开启后 VPN 的 UDP/443 流量定向进入内核已有的本机 QUIC/H3 入口，响应经原 TUN 会话返回；其他 UDP 仍直连。入口根据可见的 TLS ClientHello SNI 选择证书与目标域名，不能处理 ECH、隐藏 SNI、证书固定或不信任用户 CA 的应用。TUN 层已通过 IPv4/IPv6 UDP/443 定向与往返测试，Android Rust 双架构和调试 APK 已编译，iOS Rust FFI 本机回归通过；Android 真机与 iOS Xcode/iPhone 的 H3 流量验收均未完成。若开关开启后目标 App 的 QUIC 连接失败，可关闭开关继续使用普通 UDP 直连。
Android 手动请求与按包名添加应用的弹窗会在输入校验失败时保留原内容，允许直接修正；Java/资源编译和 APK 组装已通过，实际触屏交互待设备验收。
Android 正文详情新增上一段/下一段，按修订号与字节偏移读取完整记录；单次 JNI 只取 16 KiB，二进制段以 Base64 展示。Rust Android 双架构和 Java 已编译通过；分页按钮和跨段 UTF-8 文本显示仍需设备验收。
Android 切换默认底层网络时，服务现在会更新 `setUnderlyingNetworks` 并重建 TUN 转发会话，让旧网络上的 TCP/UDP 连接关闭后由应用重新发起。共享 `tun2proxy` 已将连接任务归属 TUN 主循环，停止时取消并等待旧任务退出；模拟 TUN 的 TCP 测试确认上游代理连接在停止后关闭。2026-10-24 双架构 APK 已重新构建，7 项转发层测试通过。Wi-Fi/蜂窝实际切换与快速多次切换仍需 Android 设备验收。

Android 阻止规则现在可直接编辑现有规则的匹配方式、模式和请求方法，保留规则 ID 与启用状态；手机端规则页可选择仅抓包、仅重放或两者。共享内核已支持这三种范围，修复了 iOS 页面选择“仅重放”时无法保存的问题。Android 双架构调试 APK 已重新构建；这些界面操作和实际流量仍需设备验收。
Android 请求详情现可“复制到新请求”：按正文修订号逐段读取最多 64 KiB 的完整请求正文；方法、URL 和普通请求头同步带入，停用头保留在编辑器但不会发送，HTTP/2 伪头不作为普通头发送。缺少方法、URL、请求头数组或请求头条目，以及超过正文或请求头编辑器上限、无法无损表示的请求头或已变化的记录，都会明确报错，避免把不完整数据误发。Java 编译和双架构 APK 组装通过，实际触屏操作待设备验收。
Android “复制到新请求”已允许 64 KiB 内的二进制正文：可读 UTF-8 用文本编辑，非法 UTF-8 或含不可见控制字节的正文用 Base64 编辑，原始字节保持不变；Base64 编辑框可容纳 64 KiB 原始正文对应的编码长度。不能无损表示的请求头会明确拒绝复制。主机 JDK 字节级回归与离线调试 APK 编译通过，实际复制、编辑和发送仍待设备验收。
复制请求正文的分段读取现逐段核对偏移、总长度、下一段位置和 16 KiB 上限；记录变化或分段不连续时停止复制，不会把错位字节带入新请求。主机 JDK 覆盖跨段原字节、重叠偏移、提前结束及错误下一段位置；离线 Android APK 编译通过，设备 UI 仍待验收。
Android 请求详情默认按摘要、请求头、响应头、正文大小和说明分组显示，文字可直接选择复制；需要检查尚未单独展示的元数据时，可切换到原始 JSON。详情仍只从数据库读取不含正文的元数据，正文按页另行读取。Java 与 APK 构建通过，触屏可读性待设备验收。
Android 首页现在整体滚动，抓包应用、启动按钮和分页列表不会因小屏幕高度被固定控件挤出屏幕；翻页时回到页面顶部。按 App 选择已用应用私有偏好保存，重开界面会恢复。布局已通过 APK 构建，仍需在不同屏幕尺寸上验收。

Android 模拟器测试现新增按 App 选择的独立场景：仅允许测试 App 进入 VPN，检查它的原始 HTTP 请求完成并写入抓包记录；同时从未选中的 HTTP Capture 进程向同一宿主服务发请求，检查服务收到请求但抓包记录没有新增。此测试与全设备模式分开运行，测试 APK 本地编译后仍需下批集中 CI 在模拟器实际执行；真机应用选择仍待验收。

按应用抓包启动时，如果任一选中的包名已经卸载或对当前应用不可见，Android 现会拒绝启动并显示对应包名，避免跳过它后只抓到其余应用却显示“运行中”。本地 APK 编译通过；包可见性及系统 VPN 的实际行为还需在模拟器与真机验证。
保存 CA 公钥证书后，Android 界面会提示用户到系统设置中安装证书；Android 11 起不能用应用内 `KeyChain` 安装 CA，仍须用户在设置中确认。部分应用不信任用户 CA 或使用证书固定，不能保证所有 HTTPS 都可解密。
Android VPN 服务在 manifest 中声明不支持系统“始终开启 VPN”，因为当前抓包需要用户从应用界面启动。合并后的 manifest 已确认包含该声明，离线调试 APK 重编及 v2 签名校验通过；系统设置中的实际表现仍待设备验收。

2026-10-24 本机 x86_64 模拟器在 Android 系统启动前退出，不能把结果算作 APK 或 VPN 失败。2026-10-25 进一步确认：把模拟器的 `lib64` 加入 DLL 搜索路径后，headless QEMU 的 `-version` 在受限工作区环境中能正常退出；用同一环境创建 API 33 AVD 并启动，QEMU 识别了系统镜像，却在创建本地 socket pair 时报告 `could not initialize hostpad event loop`。另一启动路径仍以 `0xC0000135` 退出；Windows 错误报告出现以 Codex 包装应用为宿主的 QEMU 崩溃事件。`adb devices` 没有设备。这些证据表明当前本机执行环境不能完成模拟器验收，未证明 APK 有问题；待白天在可运行的模拟器 CI 或真机验证。
Android 已接通内核的响应修改配置：可查看命中数，启停总开关/单条规则，切换仅抓包、仅重放或两者，添加精确域名、URL 包含或 URL 正则匹配；可创建整份响应状态/头/正文替换，或正文文本/正则局部替换，并删除规则。保存使用配置版本号防止覆盖其他实例的更改；输入不合法时新增弹窗保留内容。移动编辑器的整份响应正文支持 UTF-8 文本或 Base64 原始字节，限 64 KiB；响应头限 16 KiB，重复字段与停用状态可编辑并保留，其他内核支持的多操作高级规则可由桌面端配置并在移动端启停。Android 双架构 Rust JNI、Java 和 APK 构建通过，规则实际命中及触屏交互仍需设备验收。
Android 响应规则现可编辑已有的整份 UTF-8 文本或 Base64 原始字节替换，以及单个正文查找替换；二进制正文编辑保留原字节，输入最多解码为 64 KiB。编辑保留原规则 ID、启用状态，以及正文修改的大小写/全部匹配等高级参数；编辑不会自动打开原本关闭的总开关。超出移动输入上限、多操作规则和不能无损回填的响应头禁用编辑入口，仍可启停或删除。离线 Java 编译与双架构调试 APK 组装通过，APK 包含 arm64-v8a/x86_64 原生库且 v2 签名校验通过；Base64 输入、保存冲突和规则命中效果仍待设备验收。
Android 的 Base64 正文输入使用独立的有界解码器；主机 JDK 回归测试覆盖任意二进制字节、空正文、64 KiB 边界、非法/残缺编码和超限拒绝，已接入手动 Android CI。已有正文即使是有效 UTF-8，只要含 NUL 等控制字符也默认用 Base64 编辑，避免文本框隐藏不可见字节；该判断已加入 JDK 测试。新增解码器后的双架构调试 APK 已在本机离线组装通过。
模拟器抓包冒烟测试另增加真实 HTTP 上游响应的二进制整份替换：临时配置仅匹配测试路径的规则，通过本机代理请求上游，检查客户端收到 201 状态和原始 `00 FF 41` 字节，最后恢复原规则。Android 测试 APK 已离线编译通过；此新增场景尚未在模拟器或设备执行，需下一批集中 CI 验证。
Android 独立测试 App 还增加普通 UDP 回环测试：经 VPN 向模拟器宿主机发送固定标记，宿主机 UDP 服务原样回显，CI 同时检查宿主机收到标记和 App 收到相同字节。它验证实际 APK 的普通 UDP 直连链路，不把这类数据误称为 HTTP 抓包。测试 APK 与探针 APK 已离线编译，宿主机 UDP 回显程序通过本地回环测试；VPN 场景尚待模拟器执行。
Android 的 VPN 授权请求现保存待启动端口与等待状态，以便授权弹窗期间 Activity 重建后继续启动；用户取消本次启动后，迟到的授权结果不会再次开启 VPN。服务启动入口也拒绝无效端口，并在新 Activity 接到授权结果时恢复抓包控件与轮询。调试 APK 已重新组装；页面重建和授权交错仍需设备验收。
Android 与 iOS 正文预览现复用共享内核的 UTF-8 分段对齐：若 16 KiB 边界切断多字节字符，会把不完整字符留给下一段，二进制仍按 Base64 展示；界面保存真实分页起点，使“上一段”返回正确位置。共享内核定向测试验证跨段字符与逐字节还原，Android/iOS 本机 Rust 测试分别 2/4 项通过；Android 双架构调试 APK 重新构建并核对打包结果，触屏分页仍待设备验收。
Android 新建请求的正文类型支持文本、JSON、URL 编码表单、Base64、原始文件和带普通字段及一个文件的 Multipart。文件通过系统文档选择器获取；选择器返回后会恢复当前方法、URL、请求头与正文，读取失败时重新打开编辑框。正文与文件按 1 MiB 上限有界读取，Multipart 自动生成边界和对应 `Content-Type`；内核按实际正文长度更新 `Content-Length`。JDK 17 本机字节级回归已验证中文 URL 表单、Multipart 边界与文件原始字节、恶意 MIME 回退及 1 MiB 上限，并接入手动 Android 构建工作流；离线 Gradle 调试 APK 编译通过，文件选择和实际发送结果尚需设备验收。暂不支持多个文件字段、可自定义文件字段名及超过 1 MiB 的流式上传。

Android 和 iOS 手动请求可选择原生自动协商、HTTP/1.1、HTTP/2 或 HTTP/3；自动协商通过 TLS ALPN 选择 HTTP/1.1 或 HTTP/2，HTTP/3 要求 HTTPS。复制已记录请求时保留这四种受支持的发送模式，遇到移动端未内置的引擎则拒绝无声降级。移动版尚未打包 Chrome/Firefox 指纹发送引擎与 httpcloak helper；选择协议不等同于模拟对应浏览器 TLS 指纹。Android 离线 APK 重编与签名校验通过；iOS 的 Swift 界面仍待 Mac 编译。

## 目标边界

Android 的 `VpnService` 只负责获得虚拟网卡与用户授权；包转发层应将可抓取的 HTTP/TLS 连接转给手机上的 `capture-core` 代理，非目标流量保持正常连通。核心拦截、规则、持久化和证书逻辑尽量由现有 Rust crate 维护。不能用 `setHttpProxy` 冒充全流量接管，因为应用可以忽略系统代理。
对照 [ProxyPin Android VPN 服务](https://github.com/wanghongenpin/proxypin/blob/main/android/app/src/main/kotlin/com/network/proxy/ProxyVpnService.kt) 与 [iOS Packet Tunnel](https://github.com/wanghongenpin/proxypin/blob/main/ios/ProxyPin/PacketTunnelProvider.swift)：它在 Android 通过 `VpnService.Builder` 配置默认路由、可选应用名单和 HTTP 代理提示，再由独立 TUN 线程解析 IP/TCP/UDP 并转给应用内代理；iOS 配置 `NEProxySettings` 的 HTTP/HTTPS 代理，开启 `ipProxy` 时再配置 IPv4 默认路由并启动包转发服务。两端上层共用 Flutter/Dart 代理内核。我们的 Android 与 iOS 已采用相同的“系统 VPN + 本机代理”基本拓扑，但共用的是 Rust `capture-core`；两端都处理 TUN 原始包，不依赖应用遵守代理提示。ProxyPin 的代理提示、应用范围和绕过路由可作为交互参考，不能据此认为 HTTPS 证书固定、忽略用户 CA 或所有 UDP/IPv6 流量已经解决。
Android 原生桥接在解析本机透明代理地址失败时会停止已打开的桥接监听，避免下次启动遇到遗留端口；这一分支仍待 Android 目标交叉编译验证。
Android 手动请求头编辑器现保留复制来源中普通请求头的停用状态与重复顺序；可粘贴浏览器请求头，点“逐项启用或停用请求头”切换发送状态。自动补充正文 `Content-Type` 时忽略已停用的同名头。独立 Java 请求头解析测试与离线调试 APK 编译已通过，实际点击与发送待设备验收。
首次提交的 Android 模拟器冒烟在已有抓包记录、上游请求尚未完成时停止 VPN，导致脚本随后查不到目标服务的 `/vpn-smoke` 访问日志。instrumentation 现只检查本次新请求，并等待记录出现响应状态才停止 VPN；离线测试 APK 编译和单独运行的 [Android mobile build](https://github.com/godstealer/http-capture/actions/runs/40078795556) 均已通过，包括模拟器内的原始 VPN TCP 到目标服务及抓包记录检查。

## iOS 当前代码与构建入口

2026-10-25：iOS 手动请求新增“保存请求”和“已保存请求”。请求库列表分页读取，打开后可修改、覆盖保存或发送；请求头原始顺序、重复项、停用状态和最多 1 MiB 的二进制正文会保留。超过扩展单条消息上限的正文分段上传或读取。含 gRPC、HTTP/2 伪头、上游代理、脚本或自定义 TLS 配置的库中请求会拒绝在简化编辑器中打开，以免覆盖保存时丢失配置。Rust 桥接及测试源码已通过 Android 交叉目标离线类型检查；Swift/Xcode 编译、模拟器及 iPhone 操作尚未验证。本批改动仅在本地，未推送或发布。

iOS 请求库列表增加左滑删除和确认；扩展删除记录后会使该请求未完成的正文读取令牌失效，正文读到末段也会释放缓存。Swift 转换回归已接入 iOS 构建工作流，覆盖分段二进制正文及请求头、方法、URL 的无损校验；当前只有 Rust 桥接源码离线交叉类型检查通过，Swift/Xcode 与设备运行尚待白天 CI 和设备验收。

- `apps/mobile/ios/native` 是 Rust `staticlib`，提供 C ABI：在 Packet Tunnel 扩展中启动同一个 `capture-core` 回环代理，将 `NEPacketTunnelFlow` 的 IP 包通过有界队列送入 `tun2proxy::run`，并将生成的包写回扩展。它支持虚拟 DNS、IPv4/IPv6、非 DNS UDP 直连及每页最多 50 条的请求摘要、请求元数据、按偏移每段 16 KiB 的正文读取、CA 公钥证书读取。VPN 运行时宿主不另开代理，通过 `sendProviderMessage` 与扩展通信；停抓后宿主可读取 App Group 中的历史并编辑请求库、规则和证书，离线手动发送复用内核重放路径。
- `apps/mobile/ios/PacketTunnel` 配置 IPv4/IPv6 默认路由、虚拟 DNS 和 HTTP/HTTPS 代理提示；先启动回环代理监听，再应用系统隧道设置和读取 IP 包，失败时关闭内核，停止与启动回调交错时不重新进入读包循环；运行时每两秒检查 Rust 包转发任务，异常退出则由扩展向系统取消 VPN。`apps/mobile/ios/App` 提供 VPN 启停、端口、每页 50 条的历史请求列表与 URL 包含搜索、请求元数据、请求与响应正文前后翻段、CA 公钥证书分享、阻止规则增删与启停、多种正文格式的手动 HTTP(S) 请求入口。正文按 16 KiB 有界读取，不一次载入整份；高级重放编辑器仍未实现；基础界面此前通过 iOS Simulator 目标编译，近期离线能力与列表界面改动仍待 Mac 重编，全部界面尚未经过真机验收。
- `apps/mobile/ios/project.yml` 由 XcodeGen 生成宿主和 Packet Tunnel 扩展工程；扩展构建前 `build-native.sh` 根据 iPhoneOS/Simulator 与架构编译 Rust 静态库。Rust 本地测试已覆盖扩展启停、阻止规则，以及本机 HTTP 请求发送、记录详情、正文预览，2026-10-24 共 4 项通过；扩展消息限长 128 KiB。需要 macOS、Xcode、Rust Apple target、XcodeGen，以及能用于 Packet Tunnel 的 Apple 签名权限。当前 Windows 环境无法执行 Xcode 编译，更不能证明扩展启动、系统流量、内存限制、证书信任或 App Store 分发通过。

iOS 新建请求的正文编辑已增加文本、JSON、URL 表单、Base64、原始文件和带普通字段及单个文件的 Multipart。文件经系统文件选择器授权，最多读取 1 MiB；文本输入最多 64 KiB，最终正文最多 1 MiB。较小请求可直接通过扩展消息发送，较大请求使用带上传令牌和偏移校验的 16 KiB 分段消息，完成后再提交给内核；扩展同一时间仅暂存一份最多 1 MiB 的正文，新的上传会替换未完成的旧上传。Rust 本机测试已验证 160 KiB 二进制请求分段、错误偏移与未完成提交的拒绝、实际 HTTP POST 字节一致性，以及完整 1 MiB 分段接收和旧令牌失效；JSON 语法、表单编码、Multipart 边界/文件字节和非法 MIME 回退有独立 Swift 测试，已接入手动 iOS 构建工作流。Mac CI 已通过宿主编译与 Swift 测试，设备实际发送仍待 iPhone 验证；超过 1 MiB 的流式上传尚未实现。
分段上传取得令牌后，iOS 宿主现在允许用户取消；上传失败、偏移不一致或 VPN 状态变化时会尝试取消扩展内的临时正文，并使迟到的回调失效，避免继续分段或误提交。进入最终提交阶段后取消入口关闭，因为请求可能已交给内核发送。SwiftUI 改动尚待 Mac CI 编译和 iPhone 操作验收。
iOS 手动请求弹窗在上传阶段显示“取消并关闭”，会先取消暂存正文；请求已开始发送或进入最终提交后可明确选择“后台等待”，避免误以为关闭弹窗就撤回请求。发送返回记录 ID 时，弹窗会关闭并展示该请求详情；返回错误且没有记录 ID 时保留输入以便修改。此界面改动尚待 Mac 编译和 iPhone 操作验收。
iOS Packet Tunnel 收到 App 消息后，先有界检查大小和 UTF-8，再把内核命令放到扩展工作队列；完成时通过 Network Extension 的消息完成回调返回结果。这样手动请求或上传提交等待上游时，不在系统调用 `handleAppMessage` 的线程上同步等待；多个命令的并发仍由内核状态锁与上传令牌校验约束。Rust 回归现让上游暂缓回复手动请求，同时查询扩展状态，确认状态查询在响应放行前完成；2026-10-24 本机定向测试通过。此项 Swift 改动尚待 Mac 编译及 iPhone 长请求、停止 VPN 交错验证。接口依据：[Apple `handleAppMessage(_:completionHandler:)`](https://developer.apple.com/documentation/networkextension/netunnelprovider/handleappmessage(_:completionhandler:))。
同一 Rust 扩展回归还在手动请求等待上游响应时调用停止，并在上游放行前检查停止完成，再用相同端口重启；本机定向测试通过。该检查覆盖内核/转发任务的清理，仍需在 iPhone 验证系统停止回调与宿主界面行为。

iOS 请求详情新增“复制到新请求”。它从元数据获取请求头和正文修订号，复制普通请求头及其停用状态并保留重复项顺序；编辑器会改变首尾空格或不能表示的请求头则拒绝复制。正文按 16 KiB 分段读取，逐段核对偏移、总长度和最终结束位置。可显示的 UTF-8 正文原样进入文本编辑器，二进制或含控制字节的正文转 Base64；超过 64 KiB 或读取期间记录变化时明确拒绝，避免截断后重发。分段和头部转换已加入 Swift 独立测试，已在 Mac CI 执行，仍须在 iPhone 验证弹窗与实际请求。
iOS 请求头编辑器现保留复制来源中普通请求头的停用状态与重复顺序：停用行以前缀 `# ` 展示，可在“逐项启用或停用”中切换；发送草稿将停用字段交给共享内核，由传输层过滤。粘贴浏览器请求头时忽略请求行和 HTTP/2 伪头，非法普通头名称会指出行号。此轮 Swift 定向测试与 Mac 编译仍待运行。

Android 与 iOS 的“复制到新请求”现在与保存请求编辑共用高级字段校验：显式 HTTP/2 伪头、上游代理配置、有效脚本或自定义 TLS 参数无法由当前移动编辑器保留时，复制会说明原因并停止，避免重发时静默丢失。会被编辑器改成大写的方法也会在复制时拒绝。Android 应用及 instrumentation Java 源码离线编译通过；新增设备断言未运行，Swift 用例与 Mac 编译待集中验证。普通抓包生成的 HTTP/2 伪头仍不作为普通请求头复制。

Android 与 iOS 在复制记录为新请求时均拒绝缺少方法、URL 或请求头数组的元数据，并拒绝会被编辑器规范化的边界空格；iOS 还在复制时检查普通请求头名称是否为合法 token，避免直到发送时才发现草稿无法表示原始头。本轮 Android APK 离线构建通过；新增 Swift 用例和 iOS 界面仍待 Mac 集中构建验证。

iOS 请求详情默认按摘要、请求头、响应头、正文大小和说明分组显示；可切换原始 JSON，也可从正文预览返回摘要。独立 Swift 测试已覆盖重复/停用请求头和正文大小的格式化，并接入手动 iOS 构建工作流；Mac CI 已完成 Swift 测试与模拟器目标编译，仍需 iPhone 交互验收。

iOS 阻止规则页可切换仅抓包、仅重放或两者，可编辑已有规则的匹配方式、内容和方法，同时保留规则 ID 与启用状态；新增规则会开启总开关。保存期间禁用重复操作，失败后重新读取规则并保留表单内容供修正。Swift 界面已通过 Xcode 模拟器目标编译，仍待真机交互验收。

iOS 扩展停止时会在旧代理、TUN 转发任务和执行器全部清理完成前保持全局启动锁，避免用户快速关开 VPN 时新隧道抢先占用相同端口和数据库。系统 `stopTunnel` 回调已改为把原生清理放到串行后台队列，清理完成前拒绝重新启动，再通知系统停止完成；Rust 桥接回归已通过；[iOS mobile build](https://github.com/godstealer/http-capture/actions/runs/40067945885) 已通过模拟器 App/扩展编译、安装启动及无签名 iPhone arm64 编译，实际快速重启仍需 iPhone 验证。
iOS 宿主在系统显示“正在连接”或重连时提供“停止抓包”，阻止重复启动；停止过程中禁用按钮，待系统状态回到未连接再允许启动。Swift 界面已通过 Mac 编译，仍需 iPhone 状态切换验收。

iOS 扩展命令现接入共享内核的响应修改快照和版本校验保存，宿主可启停总开关与单条规则、设置作用范围、删除规则，并新增或编辑整份响应替换及单操作正文文本/正则局部替换。编辑保留原规则 ID、启用状态，以及正文修改未展示的大小写/全部匹配参数；超出移动输入上限、多操作规则和不能无损回填的响应头禁用编辑入口。保存基于完整配置字典修改，保留界面尚未展示的高级规则字段；消息超过 128 KiB 时给出明确错误，不发送截断配置。Rust 本机测试验证了规则保存和旧版本拒绝；已有 Swift 界面曾通过 iOS Simulator 目标编译，尚未经过设备操作验证。移动端高级规则界面仍待补齐。
iOS 整份响应替换编辑器现可逐项启停响应头，回填和保存时保留重复字段顺序与停用状态；响应伪头及不能无损回填的字段拒绝编辑。Swift 解析回归已加入手动 iOS CI，Mac 编译与 iPhone 交互仍待验证。
iOS 整份响应替换新增 Base64 原始字节输入，并允许重新编辑不超过 64 KiB 的已有二进制规则；保存前解码并按原字节重新编码，避免 UTF-8 替换字符改变响应。有效 UTF-8 中若含不可见控制字符，同样默认选 Base64 编辑。独立 Swift 回归已写入手动 iOS CI，覆盖原字节、编码错误、64 KiB 边界与控制字符；此轮 Swift 改动尚待 Mac CI 编译与 iPhone 界面验收。
iOS 原生扩展集成测试进一步验证真实透明代理链路：本机上游返回文本，经扩展的 CONNECT 入口按规则改为 203 和原始 `00 FF 41` 字节，客户端按字节核对结果。2026-10-24 本机定向 Rust 测试通过；这证明当前共享内核与扩展桥接的规则应用，iPhone 系统 VPN 流量仍需设备验收。
iOS 包适配器新增原始 IPv4/TCP 往返测试：通过 `PacketDevice` 输入 SYN/ACK 和正文，确认 `tun2proxy` 向本机代理发送 CONNECT 与载荷，再检查返回正文经 C 回调生成 IP 包；2026-10-24 本机 iOS Rust 桥接 5 项测试全部通过，[iOS mobile build](https://github.com/godstealer/http-capture/actions/runs/40071035617) 与同提交的 [Check](https://github.com/godstealer/http-capture/actions/runs/40071035614) 也已通过。该测试使用模拟回调与本机代理，仍不能代替 iPhone Packet Tunnel 的系统流量验收。后续修补了扩展启动时透明桥接或其代理地址初始化失败后的回滚，确保已启动的回环代理与执行器被清理；增加原始 IPv4 和 IPv6 的普通 UDP 往返测试，确认流量直连目标服务、经 C 回调返回，并且 IPv4 UDP 没有误进 HTTP 代理；再以原始 DNS 查询取得虚拟 A 记录，用该地址发起 TCP 连接，确认本机代理收到原始域名的 CONNECT。新增 IPv6/TCP 往返测试复用 IPv4/TCP 的 SYN、ACK、CONNECT、载荷和回包检查，本机 iOS 原生测试现为 9 项全部通过；[iOS mobile build](https://github.com/godstealer/http-capture/actions/runs/40083549905) 在 Mac 上执行同一套原生测试及 Swift 测试、模拟器启动、无签名 iPhone arm64 App/扩展编译与架构校验，全部通过，并生成模拟器包和启动截图附件。
iOS 原生扩展集成测试新增透明代理 HTTPS 检查：测试从扩展命令导出 CA 公钥，用该 CA 验证客户端 TLS 握手，经透明 CONNECT 发送 HTTPS 请求，并确认 URL 阻止规则返回 403、抓包记录保存为 blocked。定向本机测试通过；目标服务器故意不可用，因此该测试尚不证明上游 TLS、iPhone 系统证书信任或真实 Packet Tunnel 路由。
iOS 正文桥接已接受字节偏移，详情页可逐段前后浏览完整请求/响应正文；零字节正文也不会因为文本为空而自动关闭详情。若 16 KiB 边界落在 UTF-8 字符中间，桥接会将不完整字节留给下一段，界面记录实际段起点用于返回上一段；非法二进制仍以 Base64 显示。Rust 桥接测试用 20,000 字节响应和跨段 UTF-8 字符验证连续读取与末段，负偏移被拒绝，2026-10-24 本机 4 项测试通过。Swift 界面已通过 Xcode 编译，大正文实际内存占用仍需设备验证。
iOS 历史记录已接通内核的 URL 包含过滤；搜索时重置分页游标，前后翻页沿用筛选条件。Rust 桥接测试覆盖命中与未命中请求。模拟器工作流现固定使用 Mac 构建机架构，并校验宿主及扩展可执行文件的架构；该工作流已运行并通过。
iOS 抓包首页连接期间每两秒轮询最新一页请求。列表现改为每页最多 50 条，提供上一页和下一页；仅在最新页自动刷新，查看旧页时保持页内容与游标稳定，返回最新页或手动刷新时重新读取。这样新请求涌入时不会因为截断已加载的 500 条列表而跳过中间记录，且界面内存有界。手动搜索、翻页或断开时会废弃过期轮询结果，避免旧筛选条件覆盖新列表。此轮 Swift 改动仍待 Mac 编译与设备界面验收。

在 macOS 上先安装 Xcode、Rust 和 XcodeGen，然后执行 `cd apps/mobile/ios && xcodegen generate`，用 Xcode 打开生成的 `HTTPCaptureMobile.xcodeproj`，为宿主与扩展选择同一个 Apple Team 及支持 Packet Tunnel 的配置。首次构建会运行 `build-native.sh` 编译对应架构的 Rust 静态库。另有在开发分支的移动端文件更新时自动触发的 **iOS mobile build** 工作流，用 Mac 构建机按宿主架构构建无签名模拟器 App 与 Packet Tunnel 扩展，并检查两个可执行文件的架构；已执行并通过，但不能代替 iPhone 的签名、VPN 权限和流量验收。只有 Xcode 构建、签名和 iPhone 真机 VPN 测试通过后，iOS 才能算可交付。

已添加 Mac 上的签名设备构建入口：`bash scripts/build-ios-device.sh TEAM_ID [DEVICE_UDID]`。脚本生成 Xcode 工程，以指定 Apple Team 对宿主和 Packet Tunnel 进行自动签名，分别校验签名及实际写入的 App Group、Packet Tunnel 权限；提供设备 UDID 时用 Xcode 的 `devicectl` 安装。需要在 Xcode 登录可用的开发者账号，并为两个 Bundle ID、App Group 和 Packet Tunnel 配置可用的签名权限。此脚本尚未在 Mac 和 iPhone 上执行；签名构建或安装成功也仍需手动授权 VPN 并验收抓包流量。

iOS 工作流先构建并安装无签名模拟器 App，启动后保存首页截图，再上传 `http-capture-ios-simulator` 与 `http-capture-ios-launch` 附件；随后在同一台 Mac 上额外编译无签名的 iPhone arm64 宿主与扩展并核对架构。[首次模拟器启动](https://github.com/godstealer/http-capture/actions/runs/40065417596) 已通过，截图确认首页实际显示；后续[端口输入修复运行](https://github.com/godstealer/http-capture/actions/runs/40066439156) 的截图确认默认端口显示为 `8899` 而非 `8,899`。模拟器附件不能安装到 iPhone，设备目标编译和首页截图也不能代替签名、VPN 权限与流量验收。

## 上游 CA 证书

Android 与 iOS 已接入共享证书库的“上游 CA 证书”：可以从文件导入公开 PEM/DER CA，设置精确目标域名和端口或留空适用于所有 HTTPS 目标，再启用、停用或删除。此配置只影响代理连接目标服务器时的信任；手机端 HTTPS 解密仍需要另行安装并信任抓包 CA。Android 文件上限 1 MiB；iOS 单张导入文件暂限 64 KiB，VPN 运行时通过扩展消息、停抓时通过宿主 Rust 桥接保存。已完成 Android Java 编译、arm64/x86_64 原生库编译、两种架构调试 APK 与测试 APK 组装，确认 APK 含双架构库与新 JNI 符号；[Android mobile build](https://github.com/godstealer/http-capture/actions/runs/40086909266) 已通过模拟器证书导入/启停/删除、VPN 和 HTTPS 抓取测试（`smokeStatus=ok`、`INSTRUMENTATION_CODE: -1`），并上传双架构 APK。iOS Rust 桥接当前 15 项测试通过；近期 Swift 界面改动仍待 Mac CI，真机上游私有 CA 站点验收仍待设备测试。

## iOS 分发边界

Apple 的 [TN3120](https://developer.apple.com/documentation/technotes/tn3120-expected-use-cases-for-network-extension-packet-tunnel-providers) 明确不推荐在 Packet Tunnel 扩展内托管本机监听器或代理，也不把它作为通用内容过滤器。当前扩展在本机启动 `capture-core` 代理并由 TUN 转发流量，因此模拟器编译与启动不能证明 App Store 审核适用性。现阶段按开发/测试实现继续验证；若目标是上架，需要先确认可用的 Network Extension 能力与审核边界，再决定是否改用受支持的架构。

## ProxyPin 源码参考（2026-10-24）

- Android：`ProxyVpnService` 通过 `VpnService.Builder` 建虚拟网卡、配置路由和按 App 允许/排除；Android 10 起还可用 `setHttpProxy` 指向本机代理。`ProxyVpnThread` 从 TUN 读取 IP 包，`ConnectionHandler` 在 TCP 数据中识别 HTTP 方法或 TLS ClientHello，命中则转到代理，否则按原目标直连；UDP 另行处理。网络切换时更新底层网络并重置旧连接。它不是直接把 TUN 字节交给 HTTP 代理，而是在中间维护 TCP/UDP 连接。当前源码的包解析入口只建立 IPv4 头处理，不能把 ProxyPin 的实现视为 IPv6 验收证据。
- iOS：宿主应用 `VpnManager` 用 `NETunnelProviderManager` 保存 VPN 配置并启动扩展，`ipProxy` 默认是 `false`；`PacketTunnelProvider` 总是设置 `NEProxySettings` 的 HTTP/HTTPS 代理。只有 `ipProxy=true` 才添加默认 IP 路由、启动 `ProxyVpnService`，用 `NEPacketTunnelFlow.readPackets` 处理不遵循代理的应用流量；其 `ConnectionHandler` 同样识别 HTTP/TLS 并转发，其他连接直连。因此默认模式不能等同于完整 IP 层抓包。ProxyPin 的 HTTP 代理核心主要位于 Flutter 宿主进程，Packet Tunnel 则负责包转发；我们选择在扩展内运行 Rust `capture-core`，需要额外验证 iOS 扩展的运行和内存边界。
- 请求来源：ProxyPin Android 在转发连接建立时以原始连接地址调用 `ConnectivityManager.getConnectionOwnerUid`，再由 UID 查应用包名，并用转发到代理的本地端口关联抓包记录。这个映射依赖系统返回 UID、连接尚存在以及本地端口不被错误复用；本项目若实现来源展示，应在 TUN 层保留原始五元组并验证这种关联，而不能只读代理收到的回环连接。
- 对本项目：Android 已采用系统 VPN + TUN 转发 + 本机 `capture-core` 的同类链路；iOS 同时配置系统代理提示和默认 IP 路由，并由共享 Rust `tun2proxy` 包处理，避免维护两套 TCP 状态机。ProxyPin 采用 Apache-2.0 许可；若复制其源码，需要保留相应许可和归属。

参考源码：[Android VpnService](https://github.com/wanghongenpin/proxypin/blob/main/android/app/src/main/kotlin/com/network/proxy/ProxyVpnService.kt)、[Android 包处理](https://github.com/wanghongenpin/proxypin/blob/main/android/app/src/main/kotlin/com/network/proxy/vpn/ConnectionHandler.kt)、[Android 请求来源](https://github.com/wanghongenpin/proxypin/blob/main/android/app/src/main/kotlin/com/network/proxy/vpn/util/ProcessInfoManager.kt)、[iOS Packet Tunnel](https://github.com/wanghongenpin/proxypin/blob/main/ios/ProxyPin/PacketTunnelProvider.swift)、[iOS VPN 管理](https://github.com/wanghongenpin/proxypin/blob/main/ios/Runner/VpnManager.swift)。

待完成顺序：

1. 在 Android 真机安装 arm64-v8a APK；补验本机代理 HTTP/HTTPS、VPN TCP/DNS、停止清理与 CA 证书安装。模拟器已有部分 VPN、HTTPS 与证书导入测试通过，不能代替真机。
2. 验证前台 `VpnService` 的实际流量与生命周期，重点测试 IPv4/IPv6 TCP、双栈 DNS、普通 UDP 直连和代理自身回环；快速停止再启动、VPN 权限撤销、页面重建及 Wi-Fi/蜂窝网络切换后的按钮、端口和新旧连接状态也需实测。切网重建逻辑已写入，但设备上还需检查未受影响的请求、旧连接释放和新连接建立。验证可选 HTTP/3 抓取开关在可信 CA、证书固定和隐藏 SNI 等场景下的行为；其他 UDP 内容仍未抓取。
3. 用真机验证已实现的按 App 选择、请求详情/正文、手动请求发送与域名/URL 阻止；补充断网/切网恢复、异常退出清理、其他规则与高级请求编辑能力，并按功能逐项验收。
4. 集中推送近期 iOS 改动后，先由 Mac CI 重编宿主与 Packet Tunnel、运行 Swift 和 Rust 测试并检查 App Group 配置；此前的通过记录不覆盖这批新代码。之后在已签名 iPhone 上验证 VPN 授权、IPv4/IPv6、DNS、普通 UDP、HTTPS 解密、离线历史/导出/请求库/规则/证书、停止与重启及扩展内存占用。高级重放编辑器与设备交互仍需继续完善。不能把 Android APK 测试视为 iOS 证据。

移动版目前不纳入桌面版 Check、发布和更新频道。Android/iOS 签名、应用商店分发、证书信任限制和扩展权限需要单独处理。

设备验收需检查：授予 VPN 后 HTTP/TLS 流量进入记录；普通 UDP 保持连通但不进入 HTTP 记录；停止服务和撤销 VPN 权限后端口与 TUN 句柄释放；切到后台后通知及抓包继续运行；导出的 CA 文件只含公开证书。HTTPS 解密还取决于目标 App 是否信任用户安装的 CA，证书固定的 App 需单独处理。
