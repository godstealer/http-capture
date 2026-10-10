# 桌面预览版发布

`main` 的 push 对应 Check 全部通过后，Release 工作流检出同一提交，构建以下安装包。全部构建成功后创建 `build-<提交前12位>` 预发布版本并上传安装包、SHA256 校验文件。PR 检查不会触发发布；已发布版本重跑时不覆盖。

| 系统 | 架构 | 安装包 |
| --- | --- | --- |
| Windows | x64 | NSIS `.exe` |
| Linux | x64 | Debian `.deb` 和便携 `.AppImage` |
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
