# 模块边界与开发

功能状态、待办顺序和验收标准见 [开发路线图](roadmap.md)。

## 依赖方向

```text
React frontend ── HTTP（浏览器开发）── capture-service ─┐
               └─ IPC（桌面）──────── desktop ─────────┴─ capture-core
```

- `apps/frontend`：React 页面、请求/响应编辑器、主题、前端数据类型和统一 API 适配器。Vite 开发代理从仓库根目录读取控制令牌，令牌不下发浏览器。
- `apps/desktop/src-tauri`：窗口、平台集成及 Tauri 命令。直接调用 core，不需要单独启动 capture-service。
- `crates/capture-service`：仅负责认证、HTTP 路由和将 API 请求转换成 core 调用。监听本机回环地址，不是公网部署服务。`src/bin` 下提供独立 TCP/QUIC 抓包命令行入口。
- `crates/capture-core`：协议、MITM、发送引擎、拦截、脚本、持久化及系统凭据库。可供其他 Rust 程序引用。
- `vendor/h2`：协议库定制代码及其原始许可证，属于源码依赖，必须一起提交。不要替换成普通上游版本，否则可能丢失字段顺序能力。

目前前端类型在 `apps/frontend/src/types.ts`，Rust 模型在 core 中；修改 API 字段时需同步更新两端。HTTP 与 IPC 适配器只做参数转换，业务逻辑应放在 core。

## 命令（均从仓库根目录运行）

```sh
npm ci
npm run dev                   # 前端，127.0.0.1:1420
npm run capture               # HTTP 控制服务，127.0.0.1:1421
npm run desktop               # 桌面开发；自动启动前端
npm run build                 # 前端构建，apps/frontend/dist
npm run desktop:build         # 桌面打包，需要对应平台工具链
npm test                      # core 回归测试
npm run check:service         # 控制服务编译检查
cargo check -p http-capture-desktop
```

不要同时启动两个使用 1420 端口的前端。也可以直接进入 `apps/frontend` 执行 `npm run dev`，Vite 的令牌路径不依赖启动目录。

Rust workspace 在根目录维护 `Cargo.lock`；npm workspace 在根目录维护 `package-lock.json`，子模块无需各自安装和维护锁文件。可选浏览器发送引擎分别通过 `capture-service/browser-replay` 或 `http-capture-desktop/browser-replay` feature 启用。

Windows 需要 MSVC C++ 工具链和 WebView2，macOS 需要 Xcode Command Line Tools；Linux 的 core 凭据库需 D-Bus 开发依赖及运行时 Secret Service，Tauri 另需 WebKitGTK 等平台依赖。CI 配置覆盖三个平台的 core/服务及前端构建；具体运行结果需在 Actions 核对，尚不执行桌面安装包发布。

## GitHub 提交

提交 `apps/`、`crates/`、`helpers/`（含 Go 模组及许可）、`vendor/`、`scripts/`、`docs/`、`.github/`、根目录清单与锁文件、README 和 `.gitignore`。构建产物、依赖目录、`.local/`、CA 私钥、抓包数据库、代理配置、令牌和本地环境变量均不提交。

系统凭据库中的密码不属于仓库文件；不要把真实凭据放进测试或示例。当前 Cargo 清单声明 MIT，正式公开前应确认项目自身的授权及署名，并保留第三方源码许可证。

公开仓库已建立：[godstealer/http-capture](https://github.com/godstealer/http-capture)，默认分支 main。后续先检查差异及待提交文件，再提交和推送。

## 可选 TUN 接入

`helpers/tun` 是独立 Go 模块和进程，以应用规则筛选流量并转发至 core 专用回环 CONNECT 接口。Rust 负责配置、启动确认、状态和停止，stdin EOF 是父进程生命周期信号。构建、权限、协议边界、验证及 GPL 许可见 [TUN 文档](tun.md)。

## 本地构建空间

统一使用 Cargo 默认 target/，不再为临时测试新建 target-auto/、target-protocol/。dev/test 配置保留精简调试信息（debug=1），关闭增量缓存以降低磁盘占用，代价是重编译可能更慢。清理前停止相关进程并保留需要的安装包；.local 包含工具链、证书与运行数据，不应作为普通构建缓存整体删除。
