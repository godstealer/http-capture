# TLS 组件实测（脚本生成）

时间：2026-10-01T06:27:24.784Z

目标：H3 使用 https://tls3.peet.ws/api/all ，其他引擎使用 https://tls.peet.ws/api/all 。通过前端 /__capture/replay 调用；UA 使用用户提供的 Chrome 152。HTTP/SOCKS5 使用本机 7897 代理。本次只运行 H3 用例。

| 引擎 | 预设 | 路径 | 结果 | 响应 | 错误 |
|---|---|---|---|---|---|
| h3 | native  | HTTP | UNSUPPORTED |    | h3 send engine: HTTP/3 上游仅支持 SOCKS5 UDP ASSOCIATE；HTTP CONNECT 不支持 UDP，未回退直连 |
| h3 | native  | SOCKS5 | PASS | 200 HTTP/3 TLS 1.3 |  |
| h3 | native  | direct | FAIL |    | h3 send engine: HTTP/3 connection failed for all resolved addresses: HTTP/3 QUIC endpoint [address]: QUIC handshake timed out after 10 seconds (no HTTP response): deadline has elapsed |

PASS 表示返回 200、可解析 TLS 回显且 UA 未被改写；H3 额外验证本地协议与服务器 h3 回显。不代表完整浏览器指纹保真。H3 的普通 HTTP 上游为预期拒绝用例。GUI 点击及 TUN 未在此脚本中验证。
