# HTTP Capture 桌面版更新说明

## 更新内容

- 新增 `xhttp` 手动发送引擎。按实际 TLS 协商选择 HTTP/2 或 HTTP/1.1，保留字段顺序和重复项，并展示实际发送的头部。支持 TLS 1.2/1.3、HTTP 和 SOCKS5 代理、自定义 CA 与 mTLS。
- 抓包列表搜索覆盖请求头、响应头和响应正文。正文按需读取，界面显示进度，无法读取的正文会明确标记。
- HAR 导出补齐标准响应正文。完整模式还保留原始压缩数据，方便重新导入。
- URL 编码表单和 multipart 请求体改用可直接输入的字段表格。未录制正文明确标记，避免在查看和导出时显示为已录制的空正文。
- 新增 `install.sh`，支持 Linux x64、macOS Apple Silicon 和 Intel 从终端安装，自动选择对应包并校验 SHA256。

## 使用与验收

- 安装后在手动请求的 **TLS → 发送引擎** 选择 `xhttp`。目前使用 Go 原生 TLS，浏览器 ClientHello 预设和 H3 由其他发送引擎提供。
- 本版发布 Windows x64、Linux x64、macOS Apple Silicon 和 macOS Intel 安装包。移动端的设备验收继续进行。
- 安装、升级、真实网站兼容性及手机配对的实机验收见 [开发清单](roadmap.md)。
