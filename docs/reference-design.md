# 抓包与浏览器重放的参考设计

更新时间：2026-10-08。最初参考日期为 2026-09-22；本文描述当前采用的边界，不作为历史测试报告。

## 参考来源与用途

- [HTTP Toolkit / Mockttp](https://github.com/httptoolkit/mockttp/blob/main/src/types.ts)：参考代理生命周期、原始字段与解析字段的区分。
- [httpcloak](https://github.com/sardanioss/httpcloak)：通过独立 Go 辅助进程接入浏览器发送能力；已是可选运行时依赖，不再只是参考项目。

## 已采用的设计

保留 Tauri、React、Rust core。传输契约、浏览器版本选择、wreq 和 httpcloak 分成独立模块；core 负责脚本、拦截、代理选择及持久化。多个引擎并存，每个手动请求动态选择，能力与不可用原因由后端返回，不静默回退。

捕获输入与上游输出分开记录。H1 保留原始请求头字节和有序字段；H2 保留解码后的普通字段及伪头顺序。发送结果中的 sentRequestHeaders 表示 H1/H2 编码器输入快照，不冒充 TLS 密文或压缩后的协议字节。正文已解除 chunked 分帧，不等于原始 TCP 流。

TLS 预设不能覆盖用户编辑的 UA。可跟随 UA、显式指定版本或选择当前库支持的最新预设；不支持的版本明确报错。各引擎的保序范围与参数支持并不相同，详见 [发送引擎](transport-engines.md) 和 [协议说明](protocols.md)。

普通抓包按客户端协议使用相应原生路径；手动请求可选择 native、auto、h2、h3、wreq 或 httpcloak。wreq 需要 browser-replay 构建，httpcloak 需要 helper 和能力清单。H3 TUN 接管仍未验收，当前暂缓。

## 已有验证与待办

- 多引擎注册、并发隔离、不可用后端拒绝及 H1/H2 请求保序已有自动化测试。
- wreq/httpcloak HTTP/SOCKS5 请求、UA 保留和部分浏览器预设已有实际回显验证；原生 H3 经 SOCKS5 已通过公网测试。
- 回显成功不代表完全复制 Chrome/Firefox 的全部网络行为；各阶段证据见 [组件实测](tls-components-test.md)。
- 完整 ClientHello 扩展编辑、HTTP/2 SETTINGS/优先级控制、接收正文前的生命周期记录及分段耗时仍待完善。
- 用户已取消两引擎指纹对比项目，不恢复这项任务；保留保证功能正确性和声明能力所需的测试。
