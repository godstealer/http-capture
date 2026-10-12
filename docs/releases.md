# 桌面预览版发布

`main` 的 push 对应 Check 全部通过后，Release 工作流检出同一提交，构建以下安装包。全部构建成功后创建 `build-<提交前12位>` 预发布版本并上传安装包、SHA256 校验文件。PR 检查不会触发发布；已发布版本重跑时不覆盖。

| 系统 | 架构 | 安装包 |
| --- | --- | --- |
| Windows | x64 | NSIS `.exe` |
| Linux | x64 | Debian `.deb`、`.AppImage` 和免 FUSE `portable.tar.gz` |
| macOS Intel | x64 | `.dmg`，文件名前缀 `macos-x64` |
| macOS Apple Silicon（M 系列） | arm64 | `.dmg`，文件名前缀 `macos-arm64` |

两种 Mac 分别使用 `macos-15-intel` 和 `macos-15` runner 原生编译，包括 httpcloak Go 辅助程序；当前不生成 Universal 二合一安装包。在 Mac 的“关于本机”查看芯片类型后选择对应文件。

安装包包含原生发送引擎及 httpcloak 和其能力清单，通过 Tauri 资源目录定位辅助程序，不依赖源码目录。wreq 可选引擎和 TUN 辅助程序暂不包含。TUN H3 排查仍暂停。

预览版尚无 Windows 代码签名、macOS 签名/公证，也未实现应用内自动更新。CI 打包成功不等于实机安装、凭据库和网络功能均已验收。

本地准备资源：安装 Node、Rust、对应平台 Tauri 系统依赖和 `helpers/httpcloak/go.mod` 要求的 Go 版本后，执行：

```sh
npm ci
node scripts/prepare-release.mjs
npm run tauri -- build --config src-tauri/tauri.release.json --bundles dmg -- --locked
```

Windows 把 `dmg` 换为 `nsis`，Linux 换为 `deb,appimage`。生成的资源与配置不提交 Git。


## Linux AppImage

Linux x64 包在 Ubuntu 22.04 上原生构建，同时发布 deb 与 AppImage。AppImage 携带 WebKitGTK、媒体框架及 httpcloak 辅助程序，减少发行版包名/版本差异带来的安装问题。它仍依赖基础系统 ABI，不能保证在比构建基线更旧的 glibc 系统上运行。参考 [Tauri AppImage 文档](https://v2.tauri.app/distribute/appimage/)。

从 Release 下载 `linux-x64-*.AppImage` 后，在文件所在目录执行：

```sh
chmod +x ./linux-x64-*.AppImage
./linux-x64-*.AppImage
```

如果提示缺少 FUSE，可免挂载解包后启动（不需要 root）：

```sh
./linux-x64-*.AppImage --appimage-extract
./squashfs-root/AppRun
```

CI 同时校验两种 Linux 产物存在，并分别计算 SHA256。打包成功仍需目标发行版实机验收。


## 普通用户开箱即用方案（2026-10-12）

Linux 的 GBM/DRM 渲染兼容由程序负责，不再要求用户配置环境变量或安装 GTK 提示音模块：

- Linux 入口在 GTK/WebKit 初始化前默认设置 `WEBKIT_DISABLE_DMABUF_RENDERER=1`，绕过本次报告的 DMABUF 分配失败路径；Windows/macOS 不受影响。
- 用户显式设置该变量时保留其选择。兼容模式可能牺牲较快的渲染路径；不关闭 WebKit 沙箱、不修改显卡设备权限、不要求 root。
- 仅从当前进程继承的 GTK 模块列表中移除可选 canberra 提示音模块，保留其他模块（包括辅助功能）。由系统 GTK 配置另外加载的提示音模块仍可能产生无害警告，不作为安装依赖。
- Release 同时生成完整 AppDir 的 `portable.tar.gz`，保留共享库、媒体资源、httpcloak 和 AppRun。用户解压并运行 **AppRun** 即可，不经过 AppImage 的 FUSE 挂载。不要单独移动其中的可执行文件。
- AppImage 继续提供；没有 FUSE 且不能安装软件的用户选择 portable.tar.gz，无需运行解包参数或 apt 命令。
- 仍要求 Linux x64、Ubuntu 22.04 或兼容的更新 glibc 桌面会话；不声称能在任意 Linux、无图形会话或损坏的显卡驱动环境运行。

验收：CI 应产生 deb、AppImage、portable.tar.gz 及全部校验和；实机用普通账户解压运行 AppRun，检查窗口、编辑器、网络发送和媒体预览。已发布到 **build-c74877404b14**；尚未在用户机器验收。

依据：[Tauri Linux 图形兼容说明](https://v2.tauri.app/develop/debug/linux-graphics/)。


### c748774 产物验证记录

- Check 三平台及前端全部通过，GNOME 隔离 D-Bus 写入/恢复/冲突集成测试通过。
- Linux 构建成功，已下载本次 CI 产物核对 deb、AppImage、portable.tar.gz 的 SHA256，全部匹配。
- portable.tar.gz 含 542 个条目，AppRun 权限 0755，包含 WebKitGTK 与 httpcloak 辅助程序。
- 尚未在用户 Ubuntu 图形桌面执行 AppRun；产物完整性与桌面兼容性分别验收。

- 四个平台构建和 publish 全部成功，新预发布版已公开：[build-c74877404b14](https://github.com/godstealer/http-capture/releases/tag/build-c74877404b14)，包含 6 个安装/便携包及 4 份平台校验文件。
