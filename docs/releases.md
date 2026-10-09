# 桌面预览版发布

`main` 的 push 对应 Check 全部通过后，Release 工作流检出同一提交，构建以下安装包。全部构建成功后创建 `build-<提交前12位>` 预发布版本并上传安装包、SHA256 校验文件。PR 检查不会触发发布；已发布版本重跑时不覆盖。

| 系统 | 架构 | 安装包 |
| --- | --- | --- |
| Windows | x64 | NSIS `.exe` |
| Linux | x64 | Debian `.deb` |
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

Windows 把 `dmg` 换为 `nsis`，Linux 换为 `deb`。生成的资源与配置不提交 Git。
