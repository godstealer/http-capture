# Android / iOS 移动端

状态：Android 已重新构建包含 TUN 透明代理桥的 arm64-v8a / x86_64 双架构调试 APK，并核对包内库、JNI 入口和签名；尚未经过设备验收。iOS 宿主、Packet Tunnel 与 Rust 包桥接已实现基础链路，尚未在 Xcode 编译或设备验收。当前没有可交付的正式移动安装包。

## Android 当前代码

- `apps/mobile/android/native` 是独立 Rust `cdylib`，通过 JNI 打开 `capture-core`、启动手机本机回环 HTTP(S) 代理、读取不含正文的流量元数据以及导出 CA **公钥证书**。CA 私钥仍留在应用私有数据目录。
- `apps/mobile/android/app` 是原生 Android 工程，可输入并保存端口、选择需要抓包的应用（桌面应用可勾选，后台应用可按包名添加；VPN 启动时会校验应用是否仍已安装）、启动/停止抓包、按 URL 包含搜索、按每页最多 100 条浏览全部历史摘要、点开请求元数据并按每段 16 KiB 浏览请求/响应正文，并用系统文件选择器保存 CA 证书。首页实时刷新；搜索与旧页均使用数据库游标按需读取，页面不会一次加载全量历史。可手动新建 HTTP(S) 请求，输入方法、URL 和有序文本请求头，发送后写入现有重放记录并打开详情；正文支持文本、JSON、URL 表单、Base64 原始字节、系统文件选择及简单 Multipart 字段/单文件上传；文本最多 64 KiB，文件与最终请求正文最多 1 MiB，完整高级请求编辑器仍未实现。阻止规则页可添加精确域名、URL 包含或 URL 正则规则及可选请求方法，也可启停全局或单条规则、删除规则，复用内核校验与持久化。前台 `VpnService` 负责授权、建立 IPv4/IPv6 虚拟网卡和服务生命周期；JNI 通过 `tun2proxy` 将 TCP 与虚拟 DNS 导向本机 `capture-core` 透明代理入口。该入口识别 HTTP/TLS；非 TLS、非 HTTP 的 TCP 直接转发，包含服务端先发数据的协议，不写入 HTTP 抓包记录。TLS ClientHello 仅声明非 HTTP ALPN 时，或命中解密绕过列表时，原样转发握手；未声明 ALPN 的非 HTTP TLS 仍需专门识别和设备验收。VPN DNS 服务地址为 `10.111.0.1`，与虚拟域名地址池分开。虚拟 DNS 以独立的 IPv4/IPv6 地址池响应 A/AAAA，其他查询类型返回空答案，双栈虚拟地址均可反查原始域名。对非 DNS UDP，使用本地维护的 `tun2proxy` 小补丁直连目标，避免 HTTP 代理不支持 UDP 时阻断连接；默认情况下 UDP 内容不进入抓包记录；开启 HTTP/3 抓取后，UDP/443 可进入 QUIC/H3 入口。摘要和正文预览均走有界存储读取，详情仅在点开时载入。
- Android arm64-v8a 与 x86_64 Rust 原生库已在 Windows NDK 交叉编译并重新打入调试 APK，包含 TUN 透明代理桥。Windows 构建为 OpenSSL 启用 `no-asm`，避开 MSYS Perl 与原生 Make 的 ARM 汇编命令引号冲突；Linux NDK 仍可使用 `scripts/build-android.sh`。已确认 APK 内两架构库与本地构建结果字节一致、各有 13 个 JNI 入口，且调试 APK 签名有效；尚未在设备运行。手动触发的 `mobile-android.yml` 作为独立 Linux 构建入口，仍待首次执行。虚拟 DNS 的查询类型与双地址池回归 3 项通过。停止 VPN 后界面等待服务清理完代理和 TUN 句柄再允许重启；界面重建时会查询仍在运行的独立本机代理。服务监听非 VPN 的联网网络变化并向 Android 更新底层网络列表，退出时注销回调；现有连接在切网时是否恢复仍需设备测试。详情页已按内核实际的顶层 `FlowMetadata` 结构修正，手动请求的文本输入限制为 64 KiB，请求头最多 16 KiB；文件与最终正文最多 1 MiB。Gradle `:app:assembleDebug --offline` 于 2026-10-24 对 Java/资源部分通过。APK 启动、VPN 流量和真机行为均未验证。本机 `adb devices` 无设备；旧模拟器及新下载的官方 stable 37.2.12 模拟器的 x86_64 后端均在 Windows 加载阶段以 `-1073741515`（`0xC0000135`）退出。新版本能识别 AVD，失败发生在 Android 系统启动前；静态依赖检查没有定位到缺失的具体 DLL，不能据此归因于 APK。IPv6 路由已写入代码但未做设备验证；HTTP/3 仍待手机实测，完整的移动端规则/请求编辑界面尚未实现。

在当前已安装 Android 工具链的 Windows 工作区可运行 `scripts/build-android.ps1 debug both`；Linux 安装 Android SDK、NDK、Rust、JDK 17 和 Gradle 后运行 `bash scripts/build-android.sh debug both`。2026-10-24 本机构建的调试安装包为 `apps/mobile/android/app/build/outputs/apk/debug/app-debug.apk`，SHA-256 为 `404A2F3E71DB5A9A9C0AF685C243A7B21A8AB9E4A25EE779954DE135F09ABD07`。包内 arm64-v8a 与 x86_64 原生库已随共享内核规则更新重新交叉编译，Java/资源也已重编；调试签名校验通过。仓库的 **Android mobile build** 工作流仅支持手动触发，生成 `http-capture-android-debug` 构建附件；它不属于桌面版 Check 或正式发布流程。安装包仍需设备验收 VPN 和证书链路。

共享 TUN 转发层已修复会话计数依赖 `trace` 日志求值的问题：在普通日志级别也会更新活跃会话数；单条 TCP、DNS-over-TCP 或 UDP gateway 连接初始化失败不会结束整个 VPN 主循环，失败的 UDP 会话也会释放名额。虚拟 DNS 3 项与 TUN 回归 4 项通过：模拟 TUN 的普通 IPv4、IPv6 UDP 均可往返本机服务且内容一致，TCP 完成握手、向本机 HTTP 代理发出 CONNECT，并双向传输数据。关闭详细日志后的会话计数回归连续运行 20 次通过，TCP 往返回归连续运行 10 次通过。真实 Android 的 TUN fd、断网、切网和高并发行为仍待设备验证。
Android JNI 在发现隧道任务异常退出后，会等待回环透明代理桥释放再上报错误，避免快速重启时遗留尚在清理的监听器；双架构交叉编译与 APK 组装已通过，异常退出后的真机重启仍待验证。

Android 和 iOS 新增可选的 HTTP/3 抓取开关，默认关闭。开启后 VPN 的 UDP/443 流量定向进入内核已有的本机 QUIC/H3 入口，响应经原 TUN 会话返回；其他 UDP 仍直连。入口根据可见的 TLS ClientHello SNI 选择证书与目标域名，不能处理 ECH、隐藏 SNI、证书固定或不信任用户 CA 的应用。TUN 层已通过 IPv4/IPv6 UDP/443 定向与往返测试，Android Rust 双架构和调试 APK 已编译，iOS Rust FFI 本机回归通过；Android 真机与 iOS Xcode/iPhone 的 H3 流量验收均未完成。若开关开启后目标 App 的 QUIC 连接失败，可关闭开关继续使用普通 UDP 直连。
Android 手动请求与按包名添加应用的弹窗会在输入校验失败时保留原内容，允许直接修正；Java/资源编译和 APK 组装已通过，实际触屏交互待设备验收。
Android 正文详情新增上一段/下一段，按修订号与字节偏移读取完整记录；单次 JNI 只取 16 KiB，二进制段以 Base64 展示。Rust Android 双架构和 Java 已编译通过；分页按钮和跨段 UTF-8 文本显示仍需设备验收。
Android 切换默认底层网络时，服务现在会更新 `setUnderlyingNetworks` 并重建 TUN 转发会话，让旧网络上的 TCP/UDP 连接关闭后由应用重新发起。共享 `tun2proxy` 已将连接任务归属 TUN 主循环，停止时取消并等待旧任务退出；模拟 TUN 的 TCP 测试确认上游代理连接在停止后关闭。2026-10-24 双架构 APK 已重新构建，7 项转发层测试通过。Wi-Fi/蜂窝实际切换与快速多次切换仍需 Android 设备验收。

Android 阻止规则现在可直接编辑现有规则的匹配方式、模式和请求方法，保留规则 ID 与启用状态；手机端规则页可选择仅抓包、仅重放或两者。共享内核已支持这三种范围，修复了 iOS 页面选择“仅重放”时无法保存的问题。Android 双架构调试 APK 已重新构建；这些界面操作和实际流量仍需设备验收。
Android 请求详情现可“复制到新请求”：按正文修订号逐段读取最多 64 KiB 的完整请求正文，整体校验 UTF-8 后才进入手动请求编辑器；方法、URL 和有效的普通请求头同步带入，停用头与 HTTP/2 伪头不发送。超过正文或请求头编辑器上限、非 UTF-8 或已变化的记录会明确报错，避免把不完整数据误发。Java 编译和双架构 APK 组装通过，实际触屏操作待设备验收。
Android 请求详情默认按摘要、请求头、响应头、正文大小和说明分组显示，文字可直接选择复制；需要检查尚未单独展示的元数据时，可切换到原始 JSON。详情仍只从数据库读取不含正文的元数据，正文按页另行读取。Java 与 APK 构建通过，触屏可读性待设备验收。
Android 首页现在整体滚动，抓包应用、启动按钮和分页列表不会因小屏幕高度被固定控件挤出屏幕；翻页时回到页面顶部。按 App 选择已用应用私有偏好保存，重开界面会恢复。布局已通过 APK 构建，仍需在不同屏幕尺寸上验收。
保存 CA 公钥证书后，Android 界面会提示用户到系统设置中安装证书；Android 11 起不能用应用内 `KeyChain` 安装 CA，仍须用户在设置中确认。部分应用不信任用户 CA 或使用证书固定，不能保证所有 HTTPS 都可解密。
Android VPN 服务在 manifest 中声明不支持系统“始终开启 VPN”，因为当前抓包需要用户从应用界面启动。合并后的 manifest 已确认包含该声明，离线调试 APK 重编及 v2 签名校验通过；系统设置中的实际表现仍待设备验收。

2026-10-24 再次尝试本机 x86_64 模拟器，并关闭窗口、GPU 与硬件加速，仍在启动 QEMU 后端时以 `0xC0000135` 退出；Windows 应用错误日志没有给出相关事件。此时 Android 系统尚未启动，所以不能把结果算作 APK 或 VPN 失败；设备验收仍需真机或可运行的模拟器。
进一步确认：把模拟器的 `lib64/qt/lib` 加入 DLL 搜索路径后，非 headless QEMU 的 `-version` 能正常退出；但 headless 后端的 `-version` 和两种后端的 AVD 启动仍返回 `0xC0000135`，`adb devices` 为空。Qt 路径并非完整根因，仍不能据此判断 APK 行为。
Android 已接通内核的响应修改配置：可查看命中数，启停总开关/单条规则，切换仅抓包、仅重放或两者，添加精确域名、URL 包含或 URL 正则匹配；可创建整份响应状态/头/UTF-8 正文替换，或正文文本/正则局部替换，并删除规则。保存使用配置版本号防止覆盖其他实例的更改；输入不合法时新增弹窗保留内容。移动编辑器暂限 64 KiB 文本和 16 KiB 响应头，其他内核支持的二进制与多操作高级规则可由桌面端配置并在移动端启停。Android 双架构 Rust JNI、Java 和 APK 构建通过，规则实际命中及触屏交互仍需设备验收。
Android 响应规则现可编辑已有的整份 UTF-8 文本替换或单个正文查找替换，保留原规则 ID、启用状态，以及正文修改的大小写/全部匹配等高级参数；编辑不会自动打开原本关闭的总开关。二进制、超出移动输入上限、多操作规则和不能无损回填的响应头禁用编辑入口，仍可启停或删除。Java 编译与双架构调试 APK 组装通过，签名校验通过；编辑、保存冲突和命中效果待设备验收。
Android 的 VPN 授权请求现保存待启动端口与等待状态，以便授权弹窗期间 Activity 重建后继续启动；用户取消本次启动后，迟到的授权结果不会再次开启 VPN。服务启动入口也拒绝无效端口，并在新 Activity 接到授权结果时恢复抓包控件与轮询。调试 APK 已重新组装；页面重建和授权交错仍需设备验收。
Android 与 iOS 正文预览现复用共享内核的 UTF-8 分段对齐：若 16 KiB 边界切断多字节字符，会把不完整字符留给下一段，二进制仍按 Base64 展示；界面保存真实分页起点，使“上一段”返回正确位置。共享内核定向测试验证跨段字符与逐字节还原，Android/iOS 本机 Rust 测试分别 2/4 项通过；Android 双架构调试 APK 重新构建并核对打包结果，触屏分页仍待设备验收。
Android 新建请求的正文类型支持文本、JSON、URL 编码表单、Base64、原始文件和带普通字段及一个文件的 Multipart。文件通过系统文档选择器获取；选择器返回后会恢复当前方法、URL、请求头与正文，读取失败时重新打开编辑框。正文与文件按 1 MiB 上限有界读取，Multipart 自动生成边界和对应 `Content-Type`；内核按实际正文长度更新 `Content-Length`。JDK 17 本机字节级回归已验证中文 URL 表单、Multipart 边界与文件原始字节、恶意 MIME 回退及 1 MiB 上限，并接入手动 Android 构建工作流；离线 Gradle 调试 APK 编译通过，文件选择和实际发送结果尚需设备验收。暂不支持多个文件字段、可自定义文件字段名及超过 1 MiB 的流式上传。

Android 和 iOS 手动请求可选择原生自动协商、HTTP/1.1、HTTP/2 或 HTTP/3；自动协商通过 TLS ALPN 选择 HTTP/1.1 或 HTTP/2，HTTP/3 要求 HTTPS。复制已记录请求时保留这四种受支持的发送模式，遇到移动端未内置的引擎则拒绝无声降级。移动版尚未打包 Chrome/Firefox 指纹发送引擎与 httpcloak helper；选择协议不等同于模拟对应浏览器 TLS 指纹。Android 离线 APK 重编与签名校验通过；iOS 的 Swift 界面仍待 Mac 编译。

## 目标边界

Android 的 `VpnService` 只负责获得虚拟网卡与用户授权；包转发层应将可抓取的 HTTP/TLS 连接转给手机上的 `capture-core` 代理，非目标流量保持正常连通。核心拦截、规则、持久化和证书逻辑尽量由现有 Rust crate 维护。不能用 `setHttpProxy` 冒充全流量接管，因为应用可以忽略系统代理。

## iOS 当前代码与构建入口

- `apps/mobile/ios/native` 是 Rust `staticlib`，提供 C ABI：在 Packet Tunnel 扩展中启动同一个 `capture-core` 回环代理，将 `NEPacketTunnelFlow` 的 IP 包通过有界队列送入 `tun2proxy::run`，并将生成的包写回扩展。它支持虚拟 DNS、IPv4/IPv6、非 DNS UDP 直连及每页最多 50 条的请求摘要、请求元数据、按偏移每段 16 KiB 的正文读取、CA 公钥证书读取。C ABI 不允许宿主 App 再打开第二份代理/数据库，宿主通过 `sendProviderMessage` 与扩展通信。
- `apps/mobile/ios/PacketTunnel` 配置 IPv4/IPv6 默认路由、虚拟 DNS 和 HTTP/HTTPS 代理提示；先启动回环代理监听，再应用系统隧道设置和读取 IP 包，失败时关闭内核，停止与启动回调交错时不重新进入读包循环；运行时每两秒检查 Rust 包转发任务，异常退出则由扩展向系统取消 VPN。`apps/mobile/ios/App` 提供 VPN 启停、端口、每页 50 条的历史请求列表与 URL 包含搜索、请求元数据、请求与响应正文前后翻段、CA 公钥证书分享、阻止规则增删与启停、多种正文格式的手动 HTTP(S) 请求入口。正文按 16 KiB 有界读取，不一次载入整份；高级重放编辑器仍未实现，这些界面尚未经过 Xcode 编译和真机验证。
- `apps/mobile/ios/project.yml` 由 XcodeGen 生成宿主和 Packet Tunnel 扩展工程；扩展构建前 `build-native.sh` 根据 iPhoneOS/Simulator 与架构编译 Rust 静态库。Rust 本地测试已覆盖扩展启停、阻止规则，以及本机 HTTP 请求发送、记录详情、正文预览，2026-10-24 共 4 项通过；扩展消息限长 128 KiB。需要 macOS、Xcode、Rust Apple target、XcodeGen，以及能用于 Packet Tunnel 的 Apple 签名权限。当前 Windows 环境无法执行 Xcode 编译，更不能证明扩展启动、系统流量、内存限制、证书信任或 App Store 分发通过。

iOS 新建请求的正文编辑已增加文本、JSON、URL 表单、Base64、原始文件和带普通字段及单个文件的 Multipart。文件经系统文件选择器授权，最多读取 1 MiB；文本输入最多 64 KiB，最终正文最多 1 MiB。较小请求可直接通过扩展消息发送，较大请求使用带上传令牌和偏移校验的 16 KiB 分段消息，完成后再提交给内核；扩展同一时间仅暂存一份最多 1 MiB 的正文，新的上传会替换未完成的旧上传。Rust 本机测试已验证 160 KiB 二进制请求分段、错误偏移与未完成提交的拒绝、实际 HTTP POST 字节一致性，以及完整 1 MiB 分段接收和旧令牌失效；JSON 语法、表单编码、Multipart 边界/文件字节和非法 MIME 回退有独立 Swift 测试，已接入手动 iOS 构建工作流。当前 Windows 缺 Swift/Xcode，宿主编译和设备实际发送仍待 Mac/iPhone 验证；超过 1 MiB 的流式上传尚未实现。

iOS 请求详情新增“复制到新请求”。它从元数据获取请求头和正文修订号，只复制启用的普通请求头，并保留重复项顺序；编辑器会改变首尾空格或不能表示的请求头则拒绝复制。正文按 16 KiB 分段读取，逐段核对偏移、总长度和最终结束位置。可显示的 UTF-8 正文原样进入文本编辑器，二进制或含控制字节的正文转 Base64；超过 64 KiB 或读取期间记录变化时明确拒绝，避免截断后重发。分段和头部转换已加入 Swift 独立测试，仍须在 Mac 执行并在 iPhone 验证弹窗与实际请求。

iOS 请求详情默认按摘要、请求头、响应头、正文大小和说明分组显示；可切换原始 JSON，也可从正文预览返回摘要。独立 Swift 测试已覆盖重复/停用请求头和正文大小的格式化，并接入手动 iOS 构建工作流；当前 Windows 无 Swift 编译器，尚需 Mac 构建及 iPhone 交互验收。

iOS 阻止规则页可切换仅抓包、仅重放或两者，可编辑已有规则的匹配方式、内容和方法，同时保留规则 ID 与启用状态；新增规则会开启总开关。保存期间禁用重复操作，失败后重新读取规则并保留表单内容供修正。Swift 界面仍待 Xcode 编译和真机交互验收。

iOS 扩展停止时会在旧代理、TUN 转发任务和执行器全部清理完成前保持全局启动锁，避免用户快速关开 VPN 时新隧道抢先占用相同端口和数据库。Rust 桥接回归已通过；实际快速重启仍需 iPhone 验收。
iOS 宿主在系统显示“正在连接”或重连时提供“停止抓包”，阻止重复启动；停止过程中禁用按钮，待系统状态回到未连接再允许启动。Swift 界面仍需 Mac 编译与 iPhone 状态切换验收。

iOS 扩展命令现接入共享内核的响应修改快照和版本校验保存，宿主可启停总开关与单条规则、设置作用范围、删除规则，并新增或编辑 UTF-8 整份响应替换及单操作正文文本/正则局部替换。编辑保留原规则 ID、启用状态，以及正文修改未展示的大小写/全部匹配参数；二进制、超出移动输入上限、多操作规则和不能无损回填的响应头禁用编辑入口。保存基于完整配置字典修改，保留界面尚未展示的高级规则字段；消息超过 128 KiB 时给出明确错误，不发送截断配置。Rust 本机测试验证了规则保存和旧版本拒绝；Swift 界面尚未经过 Xcode 编译或设备操作验证。移动端高级规则界面仍待补齐。
iOS 正文桥接已接受字节偏移，详情页可逐段前后浏览完整请求/响应正文；零字节正文也不会因为文本为空而自动关闭详情。若 16 KiB 边界落在 UTF-8 字符中间，桥接会将不完整字节留给下一段，界面记录实际段起点用于返回上一段；非法二进制仍以 Base64 显示。Rust 桥接测试用 20,000 字节响应和跨段 UTF-8 字符验证连续读取与末段，负偏移被拒绝，2026-10-24 本机 4 项测试通过。Swift 界面及大正文实际内存占用仍需 Xcode 和设备验证。
iOS 历史记录已接通内核的 URL 包含过滤；搜索时重置分页游标，后续“加载更多”沿用筛选条件。Rust 桥接测试覆盖命中与未命中请求。模拟器工作流现固定使用 Mac 构建机架构，并校验宿主及扩展可执行文件的架构；仍须实际运行该工作流。

在 macOS 上先安装 Xcode、Rust 和 XcodeGen，然后执行 `cd apps/mobile/ios && xcodegen generate`，用 Xcode 打开生成的 `HTTPCaptureMobile.xcodeproj`，为宿主与扩展选择同一个 Apple Team 及支持 Packet Tunnel 的配置。首次构建会运行 `build-native.sh` 编译对应架构的 Rust 静态库。另有仅手动触发的 **iOS mobile build** 工作流，用 Mac 构建机按宿主架构构建无签名模拟器 App 与 Packet Tunnel 扩展，并检查两个可执行文件的架构；尚未执行，且即使通过也不能代替 iPhone 的签名、VPN 权限和流量验收。只有 Xcode 构建、签名和 iPhone 真机 VPN 测试通过后，iOS 才能算可交付。
手动 iOS 工作流先上传 `http-capture-ios-simulator`，内含完整的无签名模拟器 App 与 Packet Tunnel 扩展；随后在同一台 Mac 上额外编译无签名的 iPhone arm64 宿主与扩展并核对架构。模拟器附件不能安装到 iPhone，设备目标编译也不能代替签名和 VPN 权限验收。工作流仍待首次执行。

## ProxyPin 源码参考（2026-10-24）

- Android：`ProxyVpnService` 通过 `VpnService.Builder` 建虚拟网卡、配置路由和按 App 允许/排除；Android 10 起还可用 `setHttpProxy` 指向本机代理。`ProxyVpnThread` 从 TUN 读取 IP 包，`ConnectionHandler` 在 TCP 数据中识别 HTTP 方法或 TLS ClientHello，命中则转到代理，否则按原目标直连；UDP 另行处理。网络切换时更新底层网络并重置旧连接。它不是直接把 TUN 字节交给 HTTP 代理，而是在中间维护 TCP/UDP 连接。当前源码的包解析入口只建立 IPv4 头处理，不能把 ProxyPin 的实现视为 IPv6 验收证据。
- iOS：宿主应用 `VpnManager` 用 `NETunnelProviderManager` 保存 VPN 配置并启动扩展，`ipProxy` 默认是 `false`；`PacketTunnelProvider` 总是设置 `NEProxySettings` 的 HTTP/HTTPS 代理。只有 `ipProxy=true` 才添加默认 IP 路由、启动 `ProxyVpnService`，用 `NEPacketTunnelFlow.readPackets` 处理不遵循代理的应用流量；其 `ConnectionHandler` 同样识别 HTTP/TLS 并转发，其他连接直连。因此默认模式不能等同于完整 IP 层抓包。ProxyPin 的 HTTP 代理核心主要位于 Flutter 宿主进程，Packet Tunnel 则负责包转发；我们选择在扩展内运行 Rust `capture-core`，需要额外验证 iOS 扩展的运行和内存边界。
- 请求来源：ProxyPin Android 在转发连接建立时以原始连接地址调用 `ConnectivityManager.getConnectionOwnerUid`，再由 UID 查应用包名，并用转发到代理的本地端口关联抓包记录。这个映射依赖系统返回 UID、连接尚存在以及本地端口不被错误复用；本项目若实现来源展示，应在 TUN 层保留原始五元组并验证这种关联，而不能只读代理收到的回环连接。
- 对本项目：Android 已采用系统 VPN + TUN 转发 + 本机 `capture-core` 的同类链路；iOS 同时配置系统代理提示和默认 IP 路由，并由共享 Rust `tun2proxy` 包处理，避免维护两套 TCP 状态机。ProxyPin 采用 Apache-2.0 许可；若复制其源码，需要保留相应许可和归属。

参考源码：[Android VpnService](https://github.com/wanghongenpin/proxypin/blob/main/android/app/src/main/kotlin/com/network/proxy/ProxyVpnService.kt)、[Android 包处理](https://github.com/wanghongenpin/proxypin/blob/main/android/app/src/main/kotlin/com/network/proxy/vpn/ConnectionHandler.kt)、[Android 请求来源](https://github.com/wanghongenpin/proxypin/blob/main/android/app/src/main/kotlin/com/network/proxy/vpn/util/ProcessInfoManager.kt)、[iOS Packet Tunnel](https://github.com/wanghongenpin/proxypin/blob/main/ios/ProxyPin/PacketTunnelProvider.swift)、[iOS VPN 管理](https://github.com/wanghongenpin/proxypin/blob/main/ios/Runner/VpnManager.swift)。

待完成顺序：

1. 在 Android 设备或模拟器上安装含 arm64 原生库的 APK；验证本机代理的 HTTP 和 HTTPS 回环请求、VPN TCP/DNS 转发、停止清理与 CA 证书仅导出公钥。
2. 验证前台 `VpnService` 的实际流量与生命周期，重点测试 IPv4/IPv6 TCP、双栈 DNS、普通 UDP 直连和代理自身回环；快速停止再启动、VPN 权限撤销、页面重建及 Wi-Fi/蜂窝网络切换后的按钮、端口和新旧连接状态也需实测。切网重建逻辑已写入，但设备上还需检查未受影响的请求、旧连接释放和新连接建立。验证可选 HTTP/3 抓取开关在可信 CA、证书固定和隐藏 SNI 等场景下的行为；其他 UDP 内容仍未抓取。
3. 用真机验证已实现的按 App 选择、请求详情/正文、手动请求发送与域名/URL 阻止；补充断网/切网恢复、异常退出清理、其他规则与高级请求编辑能力，并按功能逐项验收。
4. Android 真机验收仍需完成；iOS 工程入口已建立，下一步在 macOS 生成 Xcode 工程并修复 Apple target 编译/链接问题，然后在 iPhone 上验证 VPN 授权、IPv4/IPv6、DNS、普通 UDP、HTTPS 解密、停止/重启与扩展内存占用。继续补齐 iOS 规则、正文、重放等移动界面。不能把 Android APK 测试视为 iOS 证据。

移动版目前不纳入桌面版 Check、发布和更新频道。Android/iOS 签名、应用商店分发、证书信任限制和扩展权限需要单独处理。

设备验收需检查：授予 VPN 后 HTTP/TLS 流量进入记录；普通 UDP 保持连通但不进入 HTTP 记录；停止服务和撤销 VPN 权限后端口与 TUN 句柄释放；切到后台后通知及抓包继续运行；导出的 CA 文件只含公开证书。HTTPS 解密还取决于目标 App 是否信任用户安装的 CA，证书固定的 App 需单独处理。
