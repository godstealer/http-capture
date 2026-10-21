# 桌面正式版发布

当前正式版见[0.1.33 自动响应重写与按需正文更新说明](release-2026-10-20-body-preview.md)。正式版 [0.1.33 / build-a59befecdc1c](https://github.com/godstealer/http-capture/releases/tag/build-a59befecdc1c)，来源 `a59befecdc1cc4b35d134baad51195c7d149210b`；[Check #33](https://github.com/godstealer/http-capture/actions/runs/39452537340) 三系统及前端全部通过，[Release #33](https://github.com/godstealer/http-capture/actions/runs/39526059061) 四平台构建、各平台 10 项原生更新生命周期测试和公开发布全部成功。公开 22 个附件已下载独立核验，17 项 SHA256 一致，五个更新包的正文及版本注释签名公钥验证通过，兼容更新清单一致。 GitHub Release 正文及应用内“更新内容”分别读取该文档及其不可变摘要；下一版须同步 `scripts/release-updater.mjs` 的说明路径。上一正式版见[0.1.31 更新说明](release-2026-10-20.md)，更早的[0.1.28](release-2026-10-19.md)与[预览版](release-2026-10-16.md)记录继续保留。真实安装与跨版本更新不由构建和验签推导为完成。

本轮已交付自动响应重写、P0 历史/正文后端接口及前端接入，范围以[本轮正式说明](release-2026-10-20-body-preview.md)为准。原 `d2d1ed7` 来源 Check #32 attempt 3 因 macOS runner 未分配最终失败，Release #29 已跳过；[原七条摘要](release-2026-10-20-response-rewrite.md)继续作为未发布来源记录，未被本版十条摘要覆盖。

`main` 的 push 对应 Check 全部通过后，Release 工作流检出同一提交，构建以下安装包。全部构建成功、签名更新包完整性检查通过后，创建 `build-<提交前12位>` 正式版本并上传安装包、更新包与 SHA256 校验文件，按版本顺序推进应用更新清单及 GitHub Latest。PR 检查不会触发发布；已公开的 build 资产重跑时不覆盖。

| 系统 | 架构 | 安装包 |
| --- | --- | --- |
| Windows | x64 | NSIS `.exe` |
| Linux | x64 | Debian `.deb`、`.AppImage` 和免 FUSE `portable.tar.gz` |
| macOS Intel | x64 | `.dmg`，文件名前缀 `macos-x64` |
| macOS Apple Silicon（M 系列） | arm64 | `.dmg`，文件名前缀 `macos-arm64` |

两种 Mac 分别使用 `macos-15-intel` 和 `macos-26` runner 原生编译，包括 httpcloak Go 辅助程序；Check 与 Release 的 ARM 标签保持一致，四个平台的原生更新测试门槛继续保留。这里的标签表示构建宿主，不把宿主版本等同于应用最低支持版本，也不据此认定目标 Mac 已验收。当前不生成 Universal 二合一安装包；在 Mac 的“关于本机”查看芯片类型后选择对应文件。

安装包包含原生发送引擎及 httpcloak 和其能力清单，通过 Tauri 资源目录定位辅助程序，不依赖源码目录。wreq 可选引擎和 TUN 辅助程序暂不包含。TUN H3 排查仍暂停。

桌面应用支持检查更新、下载安装和安装后重启。更新包使用 Tauri updater 签名；这与 Windows 代码签名、macOS 签名/公证分别配置，目前后两项仍未设置。CI 打包成功不等于实机安装、凭据库和网络功能均已验收。

## 签名更新与正式版兼容频道（2026-10-18）

安装包版本采用 `主版本.次版本.<源 Check run_number>`。主/次版本读取 `tauri.conf.json`，当前得到 `0.1.N`；`N` 来自成功的 **Check**，不使用 Release 的 run number 或重跑次数。同一次 Check 重跑、四个平台构建的版本完全相同。已公开的 `build-<sha12>` 版本和资产不覆盖；重跑时以已发布的 `latest.json` 为准，避免新生成的随机签名指向旧包。

应用继续读取固定端点：[兼容频道 latest.json](https://github.com/godstealer/http-capture/releases/download/updater-preview/latest.json)。旧安装包已写入此地址，因此保留 `updater-preview` 标签。该频道仅存放当前完整清单，仍为预发布且设置 `latest=false`；清单下载地址固定指向对应 `build-<sha12>` 的正式资产。正式 build 以 `prerelease=false` 公开，合格的新版本再设为 GitHub Latest，标题为 `HTTP Capture <版本>`。

四个原生 runner 需要生成五个更新目标，Linux 根据安装格式选择对应资产，不能把 AppImage 当作 Debian 包安装：

| 清单 platform 键 | 更新资产 | 签名 |
| --- | --- | --- |
| `windows-x86_64` | NSIS `.exe` | `.exe.sig` |
| `linux-x86_64` | `.AppImage` | `.AppImage.sig` |
| `linux-x86_64-deb` | `.deb` | `.deb.sig` |
| `darwin-x86_64` | `.app.tar.gz` | `.app.tar.gz.sig` |
| `darwin-aarch64` | `.app.tar.gz` | `.app.tar.gz.sig` |

macOS 的 DMG 继续供首次安装，更新使用整个应用归档。Linux 解包后的 portable/AppDir 不支持直接套用 AppImage 更新，应手动下载安装新 portable 包。已有未接入 updater 的旧版本需要先手动安装一次带更新功能的新版本。

CI 配置要求：

- 公钥在已提交的 `tauri.conf.json` 的 `plugins.updater.pubkey` 中，打包覆盖配置不删除或替换该值。GitHub Variable `TAURI_UPDATER_PUBLIC_KEY` 可选；若设置，必须与提交中的公钥一致。
- GitHub Actions Secrets 必须设置 `TAURI_SIGNING_PRIVATE_KEY` 和 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`，使用对应公钥的加密私钥。缺失时 preflight 明确失败，不发布无签名更新。私钥和密码不写进仓库、清单或构建命令参数。
- 锁定的 Tauri CLI 2.11.5 会自动给 NSIS、AppImage、Deb 和 macOS updater 归档签名，并把版本写进签名 trusted comment。`requireSignedVersion` 要求此版本与更新清单一致。CLI 仅警告密钥不匹配，因此收集脚本另外检查签名 key id 与公钥一致。
- `collect-release.mjs` 检查每个平台的包和 `.sig`，保留各平台独立文件名，生成构建身份、SHA256 和元数据。`generate-updater.mjs` 在汇总后再次检查全部四个平台、五个更新目标、来源 SHA、版本、签名内容和包哈希；缺少任何一项都不生成清单。`signature` 存放 `.sig` 文件内容，不是它的路径或下载地址。
- 发布任务跨提交串行，并比较数字 SemVer。旧构建晚完成也不能将频道或 GitHub Latest 退回低版本；同版本来源或清单不同则拒绝推进。发布 build 时使用 draft，完整资产上传成功后先以正式版但 `latest=false` 公开，兼容频道更新成功后才允许提升为 Latest。发布时独立读取当前 GitHub Latest 的版本，避免前次发布部分失败造成两个指针不一致。早期没有更新清单的正式版本仅接受明确的 `HTTP Capture <数字版本>` 标题，无法识别时拒绝覆盖。
- 已公开 build 重跑时沿用其原有清单及资产；若兼容频道已更新而最后一次 Latest 元数据修改失败，同一版本重跑可以补完 Latest，不重新上传安装包或重写签名。较旧的候选即使四平台成功，也只保留其正式下载页，不抢占更新频道或 Latest。

本地验证脚本：`node --test scripts/test-release-updater.mjs scripts/test-verify-published-release.mjs`（Node 22+）。fixtures 覆盖签名缺失、签名版本/key id、严格 Base64、Deb 独立目标、四平台完整性、版本和来源混用、文件名路径穿越、包哈希、正式版与兼容频道标记、延迟发布、Latest 部分失败恢复和不可变版本。fixtures 不代替真实系统安装。

发布后可将全部附件下载到独立目录，将兼容 `updater-preview/latest.json` 另存 `channel.json`，再运行只读校验。公钥和更新说明从指定来源提交读取；`--published-at` 使用 GitHub 源 Check 的 `created_at`，不依赖下载清单自报日期：

```sh
node scripts/verify-published-release.mjs --directory <附件目录> --version <版本> --source-sha <完整来源SHA> --published-at <源Check的UTC时间> --repository godstealer/http-capture --channel-file <channel.json路径>
```

该脚本核对 22 附件、17 项 SHA256、五个更新包的 Ed25519 正文及 trusted-comment 签名和兼容频道一致性。其余 DMG/portable 等附件只核对校验和。0.1.19 已按此流程验证通过；本地验证器后续收紧了格式解析，不回写已公开的不可变安装包。

协议和打包依据：[Tauri updater](https://v2.tauri.app/plugin/updater/)、[CLI 2.11.5 自动签名类型](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.5/crates/tauri-cli/src/bundle.rs#L222-L286)、[签名版本绑定实现](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.5/crates/tauri-cli/src/helpers/updater_signature.rs)。

本地准备发布资源：安装 Node 22+、Rust、对应平台 Tauri 系统依赖和 `helpers/httpcloak/go.mod` 要求的 Go 版本。先在当前进程环境提供 `SOURCE_SHA`（完整提交 SHA）、`SOURCE_CHECK_RUN_NUMBER`（该提交成功 Check 的编号），以及上述两个签名变量；本地值不会写回源码版本。然后执行：

```sh
npm ci
node scripts/prepare-release.mjs
npm run tauri -- build --config src-tauri/tauri.release.json --bundles app,dmg -- --locked
```

macOS 为完整发布使用 `app,dmg`，Windows 使用 `nsis`，Linux 使用 `deb,appimage`。生成的资源与配置不提交 Git。仅编辑/开发界面仍使用原有 `npm run desktop`，不需要发布签名环境。


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
