# TUN 指定应用抓包

状态：2026-10-08，Windows 指定应用 HTTP/HTTPS 有实测；TUN H3 未验收，按用户要求暂缓。后续默认推进跨平台交付，不自动恢复 H3 排查。下文保留历史诊断，早期缺失工具链等描述不是当前环境状态。

## 当前实现

Tauri / HTTP 服务通过 Rust core 管理独立的 `http-capture-tun` 辅助进程。辅助进程使用固定版本 sing-box 1.14.0，创建 TUN、查询流量所属应用并执行路由。选中应用的 HTTP / TLS 流量进入回环上的专用 CONNECT 接口，复用已有抓包、脚本、拦截、发送引擎和数据库。

应用规则支持可执行文件名称或完整路径，每行一项。完整路径更精确；Windows 路径分隔符统一处理并忽略大小写。非选中应用、未识别出所属应用的连接、非 HTTP/TLS 流量直连；内核与辅助进程强制绕过，避免循环。

**这是 TUN 内的应用分流，不是仅向某个进程注入网络钩子。** 启动会创建系统路由，其他应用的流量可能经过 TUN 后直连。本配置禁用 TUN DNS 接管，不自动设置系统代理或 CA 信任。底层 sing-tun 在 Windows system 栈启动时可能创建辅助程序的 TCP 入站允许规则；不能承诺完全不触及防火墙。诊断过程未停用防护或修改 UDP 阻止规则。

## 构建和运行

Windows：需要 Go 1.25.5 或更新版本，执行 `scripts/build-tun.ps1`。已存在项目内 `.local/go-tun/go/bin/go.exe` 时优先使用它；否则使用 PATH 中的 Go。辅助进程输出到 `.local/tun/http-capture-tun.exe`。

macOS / Linux：在 `helpers/tun` 执行 `go build -mod=readonly -trimpath -o ../../.local/tun/http-capture-tun .`。需要对应平台工具链。发布时将辅助进程放在桌面/服务可执行文件旁边，开发模式也会查找 `.local/tun`。目前未自动加入桌面安装包。

1. 用管理员权限（Windows）或 root/所需网络权限（macOS/Linux）运行内核/桌面端。当前没有自动提权服务；普通用户启动失败时会返回错误。
2. 打开「代理设置 → TUN · 指定应用」。填写例如 `chrome.exe` 或应用绝对路径。
3. 如需查看 HTTPS，在目标客户端信任导出的 CA。证书固定、ECH、私有 TLS 协议不保证可解密。
4. 点击「启动 TUN」。界面只有收到辅助进程启动确认后才显示运行中。
5. 重建目标应用连接后测试；既有连接不保证被接管。
6. 使用「停止 TUN」退出；停止普通抓包代理不会停止 TUN，两者是独立监听器。

## 协议边界

- 已支持 CONNECT 内明文 HTTP，并从 Host 恢复 URL；TLS 从 SNI 恢复证书域名和上游域名。无域名时保留目标 IP。
- 复用已有 HTTP/1、HTTP/2 MITM 逻辑，仍遵守正文缓冲和超时限制。
- 已加入可选 H3 路径：选中应用的 UDP 443 经 QUIC 嗅探后重定向至回环解密入口，以可见 SNI 签发证书并恢复目标域名，复用 core 执行流程。该路径的本地解密测试通过，但 Windows TUN 实机仍握手超时，**尚未验收**。不支持 ECH、无 SNI、非 443 端口、跨域连接复用及原始目标 IP 保留。默认 UDP 仍直连；阻止 QUIC 和解密 H3 为互斥选项。
- TLS 协议识别不代表其中一定承载 HTTP；其他 TLS 应用可能无法经 HTTP 解析器处理。
- 本机回环流量默认排除；不能将此功能当作所有本机通信的抓包器。
- DNS 不解密、不作为 HTTP 请求显示。应用进程识别失败时不会捕获该连接。

## 生命周期

Rust 持有辅助进程的 stdin 管道作为存活凭证。正常停止时关闭管道，辅助进程关闭引擎并清理自身的 TUN/路由；主程序崩溃后操作系统关闭管道，也会触发辅助进程退出。启动失败会关闭桥接监听器并等待清理。退出超时不强杀正在清理路由的辅助进程，界面保留停止重试入口。

尚不能宣称覆盖断电、辅助进程自身被强杀、系统路由被其他 VPN 同时修改等情况；没有实现持久化恢复日志或专用修复服务。辅助进程不会自动后台重启。

## 验证

- Rust 本地 HTTP/HTTPS 转发测试覆盖 CONNECT 明文、SNI 证书、头顺序及上游证书验证。
- `scripts/rust.ps1 test --locked -p capture-core --test tun_helper '--' --ignored`：真实辅助进程接受 Rust 生成的配置，仅执行 `--check`，不更改系统网络。
- `scripts/rust.ps1 build --locked -p capture-service --bin tun-smoke` 后，管理员执行 `scripts/test-tun-admin.ps1`：使用隔离数据目录和专用 curl 副本访问 httpbin.org，验证选中应用捕获、普通 curl 绕过及停止。结果写入 `.local/tun-smoke-result.txt`；失败也尝试清理。不会安装测试 CA，curl 仅通过 `--cacert` 信任该 CA。
- Windows curl 测试使用 `--ssl-revoke-best-effort` 处理缺失撤销信息，仍保留证书链和主机名校验；此参数不改变产品上游 TLS 校验。
- Windows 实机测试已通过：指定应用 HTTP/HTTPS 捕获、未选应用绕过、正常停止；`scripts/test-tun-parent-exit.ps1` 验证父进程直接退出后辅助进程自动结束且测试 TUN 地址清除。macOS/Linux 实机 TUN、IPv6 实际流量、GUI 点击、辅助进程自身强杀和 VPN 并存仍待验证。

## H3 待完成验证（2026-10-07）

- 本地动态 SNI 入口已覆盖多个域名、正文往返、跨域拒绝和停止清理；TUN 辅助程序接受新路由配置。此前 H2/H3 6 项回归及前端构建通过。
- 上次管理员实机测试仍在 QUIC 握手阶段超时，但正常停止后辅助进程退出、测试 TUN 地址移除均通过。不能用手动 H3 经 SOCKS5 成功代替 TUN 接管成功。
- 当前工作区 `.local` 已缺失，Rust 工具链不在 PATH，TUN 辅助程序及历史日志也缺失。旧 `target` 二进制存在，但不能据此验证新代码。
- 测试入口：重新构建 `scripts/rust.ps1 build -p capture-core --example tun-h3-smoke`，再以管理员运行 `scripts/test-tun-h3-admin.ps1`。仅接管专用测试程序，通过临时 CA 验证，不安装系统证书。
- 最新测试修正：启动前创建日志目录；保存抓包条数及协议/状态/错误摘要。`.local/tun-h3-helper.log` 用于判断进程匹配、QUIC 嗅探和路由，`.local/tun-h3-smoke-flows.json` 区分未接管与上游失败；`.local/tun-h3-smoke-result.txt` 保存执行和清理结果。日志可能包含网络元数据，仅诊断时通过 `HTTP_CAPTURE_TUN_DEBUG_LOG` 启用。
- 本轮仅 PowerShell 语法校验通过；最新 Rust 测试诊断改动因缺少工具链尚未编译。下一步恢复依赖后重跑上述实机测试，再根据日志修复路由；不要提前标记 TUN H3 完成。

### 依赖恢复后的复测（2026-10-07）

`.local` 已恢复，最新诊断程序已成功编译。管理员实机复测仍握手超时，`capturedFlows=0`；日志未出现专用测试程序的目标 QUIC 流量。运行期间 `Find-NetRoute` 确认目标 `134.209.246.126` 走 tun0 的 `128.0.0.0/1` 路由，因此不能将问题直接归因于其他 VPN 抢占路由。关闭 Quinn UDP 分段卸载的对照试验没有改善，临时开关已移除。所有本轮测试均确认辅助进程退出、测试 TUN 地址清除。下一步需要检查测试进程 UDP 发出情况与 TUN 入站处理，尚未确定丢包位置；H3 TUN 仍未验收。

### 独立客户端排查（2026-10-08）

新增 `helpers/tun/cmd/h3-test-client`，使用 Go quic-go 的 HTTP/3 客户端，仍校验临时 CA。构建该程序后，可向管理员测试脚本传入 `-Client <exe 的绝对路径>`；测试将复制它并仅选择该副本。未传参数时继续使用 Rust Quinn 客户端。

Go 客户端实机结果同样为握手超时、零捕获；辅助进程退出和 TUN 地址清理通过。这只能排除“仅 Quinn 客户端故障”的解释，尚未定位丢包。之前 Quinn 底层发送返回成功，目标 UDP 443 的 pktmon 记录未显示数据包，不能据此断言防火墙是根因。

复查发现测试依赖的 SOCKS5 `127.0.0.1:7897` 当前未监听。测试已增加启动 TUN 前的 TCP 可达性检查（不代表 SOCKS5 UDP 能力检查），并先重置结果文件，防止误读旧测试结果。恢复上游后仍需继续定位客户端到 TUN 的握手问题；TUN H3 不应标为完成。

## 隔离外部依赖的 H3 测试（2026-10-08）

`scripts/test-tun-h3-admin.ps1 -Client <Go 测试客户端绝对路径> -LocalEcho` 新增本地回显模式。Go 客户端向 `198.18.0.10:443` 发起真实 QUIC 连接，保留 `tls3.peet.ws` 的 SNI 和临时 CA 校验；此地址用于测试，必须由 TUN 接管。Rust 使用仅存在于测试示例的回显发送引擎，不连接外部网站、不依赖 SOCKS5。因此成功只代表“选中应用 → TUN → H3 解密 → 本地响应”，不代表公网转发成功。生产发送引擎不受影响。

本轮 Windows 管理员测试：默认 system 栈和启用 `with_gvisor` 的默认 mixed 栈均握手超时，零捕获；两次均确认辅助进程退出、TUN 地址移除。尚不能确定 UDP 丢包位置，不能宣称 TUN H3 可用。H2/H3 传输及动态 SNI 入口共 7 项回归通过。下一步应检查 Windows UDP 发出到虚拟网卡入口这一段，而非继续以外部站点可达性解释本地测试失败。

Go 客户端构建（项目根目录，已安装兼容 Go 工具链）：

```sh
go -C helpers/tun build -mod=readonly -o ../../.local/tun/h3-test-client.exe ./cmd/h3-test-client
```

`-LocalEcho` 不带 `-Client` 会在启动前报错。普通模式仍使用原有公网端到端测试。管理员测试会重置结果文件并验证清理；日志含网络元数据，不应上传。

## 辅助进程许可说明

`helpers/tun` 链接 sing-box，采用 GPL-3.0-or-later，许可证保存在 `helpers/tun/LICENSE`。分发辅助进程时必须一并处理对应源码、构建文件和第三方许可，不能将其标成单一 MIT 二进制。当前通过独立进程及配置协议接入现有 Rust 工程。

参考：[sing-box TUN](https://sing-box.sagernet.org/configuration/inbound/tun/)、[应用匹配规则](https://sing-box.sagernet.org/configuration/route/rule/)。

### UDP 端口对照（2026-10-08）

管理员脚本新增 -UdpProbes（须与 -LocalEcho、-Client 一起使用）。仅显式启用时向测试地址的 UDP 44444 和 443 发送普通探针；套接字保持存活，便于匹配进程。已移除 Go 测试客户端重复的 Dial 配置。

同轮实测：两个发送调用均成功，源地址均为 172.31.255.1；辅助程序日志可见 198.18.0.10:44444 并正确识别测试进程，未见同地址 UDP 443；QUIC 仍超时。说明 TUN UDP 入站和进程识别至少对对照端口有效，但仅凭日志缺失不能确定丢包具体层次。只读检查未发现直接针对 UDP 443 的 Windows 出站阻止规则，未修改防火墙。测试停止后辅助进程和 TUN 地址均清理成功。

### 排除 H3 路由规则（2026-10-08）

管理员测试新增 -ObserveUdp，必须与 -LocalEcho、-UdpProbes、-Client 一起使用。该诊断模式关闭 H3 嗅探和重定向，不预期 HTTP/3 请求成功；不能把它作为抓包验收。结果文件记录 local/observeUdp 模式，启动前清空本轮辅助程序日志，避免误读旧记录。

本轮关闭 H3 规则后，同样只观察到普通 UDP 44444，而 UDP 443 和 QUIC 超时，清理成功。只读 WFP 查询确认火绒网络过滤驱动正在运行并参与 UDP 数据处理，但没有证据确定其拦截了本次流量；需要对应产品拦截日志进一步确认。未停用、修改防火墙或驱动。WFP 原始导出保留于忽略目录 .local，不上传。

### 虚拟网卡原始收包定位（2026-10-08）

用户表示已关闭火绒后复测：现象不变，不能归因于火绒。临时诊断版在 Wintun ReceivePacket 返回后、协议栈处理前，仅对 198.18.0.10 的 IPv4 UDP 记录端口与长度。实际仅收到 UDP 44444（42 字节）；普通 UDP 443 与 QUIC 都未到达该收包点。两种发送调用均返回成功。故本轮问题发生在应用 TUN 协议处理之前；具体系统过滤组件尚未确定。已建议恢复防护，未修改防火墙/驱动。

诊断副本放在 .local 内，并以临时模组替换构建，正式依赖不变；临时辅助程序已移除。管理员测试确认 helperExited=True、tunAddressesRemoved=True。此结果不能替代 H3 接管验收。

### 多端口 UDP 对照（2026-10-08）

用户要求扩大端口测试。保持同一测试进程、目标 198.18.0.10 和 14 字节探针，关闭 H3 嗅探/重定向，直接在 Wintun 原始收包处观察。两轮结果一致；第二轮每端口发送三次：UDP 53、123、853、8443、44444 各收到 3/3；UDP 80、443 收到 0/3。所有发送调用成功。此结果仅代表到达虚拟网卡，不证明对应公网服务可达，也不证明 H3 完整往返正常。

问题不限于 UDP 443，目前涉及 UDP 80/443；具体过滤或重定向组件尚未确定。辅助进程退出、TUN 地址清理均成功，临时诊断辅助程序已移除。
