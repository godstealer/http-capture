# Header Order 实测对照

2026-10-14：Reqable（用户指定 192.168.1.18:9000）与本项目隔离代理，使用同一 Python 客户端和本地 TCP 源站，分别发送相同字段样例。两次本地源站端口不同，Host 随目标端口变化；字段排列和其他值相同。

## H1 结果

字段顺序：X-A → hOsT → X-B → x-a → Cookie → X-C → Cookie → Connection。

两款代理各自的客户端输入与源站收到的请求头字段完全一致，保留大小写、交错重复 X-A/x-a 和两条 Cookie。本项目测试实例使用临时数据库，未更改用户代理配置。请求行从 absolute-form 转为 origin-form，未将此当成保序失败。

复现：`python scripts/compare-header-order.py --proxy HOST:PORT --output .local/result.json`。本项目隔离代理：`node scripts/cargo.mjs run --locked -p capture-core --example header-order-proxy`，退出后清理临时数据库。

## 尚未证实

- 此结果是 H1 的一个固定样例，不代表所有字段、HTTPS、H2 或 H3。
- 用户截图已确认 Reqable Raw 展示的字段顺序、大小写及重复项与测试输入一致；可在 Reqable 查找 `/header-order-check`，本次目标是 `http://127.0.0.1:65080/header-order-check`。
- H2 仍需独立建立 TLS/ALPN、可信测试证书和有序 HPACK 源站观察，不用 H1 结果替代。

## 开源实现参考与独立 H2 验证

- mitmproxy 的 `Headers.fields` 保存原始字段对序列；普通字典读取可能合并重复值，赋值会替换同名字段。保序处理应使用 fields 或 items(multi=True)，而不是先转换普通字典。来源：https://docs.mitmproxy.org/stable/api/mitmproxy/http.html
- nghttp2 提供 HTTP/2/HPACK 实现；本次使用 Node 自带 nghttp2 1.47.0 的 HTTP/2 服务端，通过 stream 回调 rawHeaders 观察字段，不使用已经合并的 headers 对象。
- 本项目原生 h2 发送 → 本地 nghttp2 h2c 接收，实际观察到：`:method → :scheme → :authority → :path → x-a → x-b → x-a → cookie → x-c → cookie`。字段值也逐项一致。
- 工具：`node scripts/h2-order-observer.cjs .local/nghttp2-order.json` 输出本地地址；然后 `node scripts/cargo.mjs run --locked -p capture-core --example h2-order-client -- http://127.0.0.1:PORT/order`。比较接收 JSON 与客户端打印的 sentRequestHeaders。
- 这是独立实现之间的明文 H2 发送样例验证，不是 Reqable H2 MITM 对照，也不覆盖 TLS、H3 或所有 HPACK 情况。原有测试另行覆盖 CONTINUATION 与动态索引。
