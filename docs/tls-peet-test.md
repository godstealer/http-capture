# TLS 回显测试

使用用户提供的 tls.peet.ws/api/all GET 请求及 16 个有序请求头。请求头保持 Chrome 152 的原始值，不因选择 Firefox 或 Chrome TLS 预设而自动改写；这属于 TLS/HTTP2 预设测试，不表示整套请求与真实浏览器完全一致。

## 复现

准备请求数据（不发送）：

```powershell
./scripts/test-tls-peet.ps1 -PrepareOnly
```

独立发送内核测试（不依赖 UI 服务或拦截规则）：

```powershell
scripts/rust.ps1 run -p capture-service --bin tls-probe -- .local/tls-peet-request.json native auto
scripts/rust.ps1 run -p capture-service --features browser-replay --bin tls-probe -- .local/tls-peet-request.json chrome firefox
```

UI 服务测试：`./scripts/test-tls-peet.ps1 -Engines native,auto,chrome,firefox`，须先运行带 browser-replay 的开发服务。

每次测试发起真实网络请求，服务端将看到出口地址和请求头。结果保存在 .local/tls-peet-<engine>.json；报告不保存服务端返回的 IP/端口，但包含 TLS 和 HTTP/2 诊断。该目录不应提交 GitHub。

## 浏览器引擎构建

- `npm run capture:browser`：启用 wreq 的开发服务。
- `npm run desktop:browser`：启用 wreq 的桌面开发版。
- 默认命令仍保持原生构建，避免要求所有开发者安装 BoringSSL 工具链。
- Windows 需要 MSVC、CMake、NASM 与 libclang。scripts/rust.ps1 自动发现项目 .local/build-tools/nasm-3.02 和 .local/build-tools/clang/clang/native，不修改系统 PATH。
- NASM 便携包来自 https://www.nasm.us/pub/nasm/releasebuilds/3.02/win64/nasm-3.02-win64.zip；libclang 使用 Python wheel `libclang==18.1.1`，安装到项目 .local/build-tools/clang。

Chrome 预设固定为 Chrome 147，Firefox 固定为 Firefox 136。用户请求的 User-Agent 为 Chrome 152，保留原值以避免改变测试输入。不要将 JA3/JA4 单次回显当作真实浏览器完整保真认证。

## 本次验证（2026-09-29）

- native：HTTP 200，HTTP/1.1，TLS 1.3（回显数值 772）。
- auto：HTTP 200，h2，TLS 1.3；HEADERS 帧包含用户的 16 个字段，顺序与输入一致。
- Firefox 136：HTTP 200，h2，TLS 1.3。
- Chrome 147 默认配置：多次出现响应读取 reset、broken pipe 或 timeout；尚未验证通过。没有自动回退或改用 Firefox。

开发服务调用最初返回 Windows socket error 10013；独立获准联网进程通过相同 capture-core 测试，原生和 Firefox 成功。遵照用户选择，本次不重启正在运行的服务，GUI 仍使用原生构建。

启用浏览器 feature 时修正了历史代码对不存在的 Response.chunk() 的调用，改用 bytes_stream()，继续保持 8 MiB 限制。wreq 开启 TLS 证书信息，可返回目标证书链；该版本公开接口不提供协商版本/密码套件，UI 不伪造这两个字段，测试端点的正文仍提供其观察值。

本地验证：带 browser-replay 的 fidelity/engines 测试共 5 项通过；桌面 feature 编译通过；前端构建通过。

Chrome 诊断补充：保持原始请求头，单独关闭 GREASE 与扩展随机排列后仍在响应阶段超时；因此未采用该配置作为默认修复，也不将其归因为这两个选项。当前 Chrome 仍需进一步定位远端兼容性/网络路径与预设行为。

### 10013 开发服务联网权限修复（2026-09-29）

用户授权后，将现有 capture-service 从受限启动环境迁移为获准联网的后台进程，保持同一可执行文件及数据目录，恢复 127.0.0.1:8080 监听。未修改防火墙、TLS 校验或发送引擎，当前仍为原生构建。

通过浏览器实际使用的 1420/__capture/replay 接口验证：Auto 请求 https://httpbin.org/get?show_env=1 返回 200 / HTTP/2 / TLS 1.2，error=null。10013 不再出现。tls.peet.ws 同轮 native/auto 测试仍在 30 秒超时，不能声称该目标已恢复；这与之前立即返回 socket permission denied 的故障不同。

后续启动长期联网服务应从普通用户终端或获准联网的执行环境启动，不要从受限环境创建后台子进程。仅重新开启监听不会改变进程继承的权限。
