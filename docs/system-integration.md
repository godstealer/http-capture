# 系统代理恢复与 CA 管理

入口：顶部工具栏地球图标可切换系统代理，代理设置 → 系统代理 / CA 信任管理可查看详情。绿色勾号表示已接管，灰色叉号表示未接管，琥珀色时钟表示下次启动启用。监听时点击立即切换；停止时只改变下次启动偏好。桌面 IPC 与开发服务使用同一核心实现。

## 当前支持范围

Windows：自动接管及恢复当前用户 WinINet 默认连接的 HTTP/HTTPS 代理；读取、安装、卸载当前用户 Root 库中的本机 CA。
Linux GNOME（包括 Ubuntu 默认桌面）：接入 gsettings/dconf 的当前用户 HTTP/HTTPS 代理设置，复用快照和冲突恢复。需要 GNOME 会话、会话 D-Bus、`/usr/bin/gsettings`、`/usr/bin/dconf` 及默认 dconf 后端；缺少依赖或非 GNOME 桌面保持手动模式。
macOS：系统代理仍需手动配置。macOS/Linux 的 CA 自动信任管理尚未完成，继续使用证书详情与导出入口。
不修改 WinHTTP、VPN 独立连接、Firefox 独立信任库、其他设备或防火墙。Linux 设置仅作用于遵循 GNOME 代理配置的应用，不修改终端代理环境变量。

## 系统代理

1. “启动抓包时同时设置为系统代理”默认关闭，每次打开应用默认仅监听；可随时勾选，下一次启动抓包时才接管，不影响当前代理状态。运行中仍可点“设为系统代理”或“恢复原代理”单独切换。启用失败会明确提示，监听保持运行。
2. 接管前保存原代理服务器、绕过列表、PAC 地址及自动发现标志，再写入监听地址（通配地址转换为回环地址）。
3. “恢复原代理”、停止抓包、正常退出会尝试恢复；恢复后再次读取校验。
4. 崩溃/强制结束无法执行退出代码：**下次启动时恢复**，尚无独立看护进程即时恢复。紧急时可以在 Windows 代理设置中手动关闭 HTTP Capture 的代理。
5. 桌面与开发服务共享 `%LOCALAPPDATA%/http-capture/system-proxy/recovery.json` 及文件锁，防止互相接管。
6. 只有当前配置仍等于本工具写入值时才回滚；外部软件修改、损坏快照或恢复失败会保留快照并显示错误，不强制覆盖。冲突时需要核对 Windows 代理设置与快照。快照可能包含内部 PAC 地址，不应上传。

## CA

展示本机实际 PEM 公钥证书的主题、有效期、SHA-256 和当前用户证书库安装状态。安装状态不是所有应用均信任的保证。
安装/卸载需在界面明确确认，操作前核对展示的指纹，操作后重新查询证书库。
按完整 DER 匹配删除，不按通用名称删除，保留磁盘 CA 和私钥；不会移除其他实例签发的同名证书。
不自动安装证书，不上传/导出私钥。Linux、macOS 与远程客户端仍按各自信任库要求安装导出的 CA。

## 验证与限制

自动测试覆盖原代理/PAC 序列化、进程互斥、模拟崩溃恢复、外部修改冲突、恢复失败/读回不一致、损坏快照及证书指纹变化拒绝。
Windows 测试仅只读访问实际 WinINet 设置与根证书库；未在开发者真实系统中执行代理切换或信任安装/卸载。
参考：[WinINet per-connection options](https://learn.microsoft.com/en-us/windows/win32/api/wininet/ns-wininet-internet_per_conn_optiona)。

## Linux GNOME 实现与验收（2026-10-12）

- 通过官方 GNOME schema 读取有效配置，以单次 `dconf load /system/proxy/` changeset 写入；不逐条切换模式、主机和端口，避免中间配置状态。
- 接管前保存 HTTP/HTTPS 主机端口、模式、PAC、绕过列表、认证开关、共享代理开关及 FTP/SOCKS 主机。抓包期间关闭旧代理认证、共享及 FTP/SOCKS 主机，避免把旧认证或其他协议发给抓包入口。原认证用户名/密码不读取、不落盘、不修改。
- PAC 地址和绕过列表保留；恢复保存的有效值（原先继承默认值的键会成为同值显式设置）。仅写白名单字段，不重置整个 dconf 树。
- Linux 快照目录为 `$XDG_STATE_HOME/http-capture/system-proxy`，未设置时为 `$HOME/.local/state/http-capture/system-proxy`，权限 0700。重启后恢复逻辑与 Windows 共用。
- Windows 上已运行跨平台配置生成、快照兼容、注入拒绝和恢复测试。本机无可用 WSL，**尚未完成 Ubuntu 桌面实测**。
- 新增 `scripts/test-gnome-proxy.sh`，CI 在独立 D-Bus 会话、临时 XDG 配置及独立 dconf profile 中测试真实写入、模拟崩溃恢复与外部修改冲突。新增 CI 尚未推送/运行，不等同于已通过 Linux 实测。
- macOS 系统代理与 macOS/Linux CA 管理继续列为待办。

协议依据：[GNOME 代理 schema](https://github.com/GNOME/gsettings-desktop-schemas/blob/master/schemas/org.gnome.system.proxy.gschema.xml.in)、[dconf 工具实现](https://github.com/GNOME/dconf/blob/master/bin/dconf.c)。
