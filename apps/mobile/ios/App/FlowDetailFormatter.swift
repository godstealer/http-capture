import Foundation

enum FlowDetailFormatter {
    static func format(_ flow: [String: Any]) -> String {
        let request = flow["request"] as? [String: Any] ?? [:]
        let response = flow["response"] as? [String: Any]
        let method = request["method"] as? String ?? "请求"
        let url = request["url"] as? String ?? ""
        var lines = ["\(method) \(url)", "", "状态：\(flow["state"] as? String ?? "未知")"]
        if let status = response?["status"] as? Int {
            lines[2] += " · HTTP \(status)"
        }
        let duration = flow["durationMs"] as? Int ?? 0
        lines.append("耗时：\(duration) ms")
        if let protocolName = flow["clientProtocol"] as? String, !protocolName.isEmpty {
            lines.append("客户端协议：\(protocolName)")
        }
        if let origin = flow["trafficOrigin"] as? [String: Any] {
            var source = "来源：\(origin["entry"] as? String ?? "未知")"
            if let process = origin["process"] as? [String: Any],
               let name = process["name"] as? String, !name.isEmpty {
                source += " · \(name)"
            }
            lines.append(source)
        }
        if let error = flow["error"] as? String, !error.isEmpty {
            lines.append("错误：\(error)")
        }
        appendHeaders("请求头", side: request, into: &lines)
        if let response { appendHeaders("响应头", side: response, into: &lines) }
        if let bodies = flow["bodyReferences"] as? [String: Any] {
            lines += ["", "正文大小"]
            for (key, label) in [("request", "请求"), ("response", "响应")] {
                if let body = bodies[key] as? [String: Any],
                   let bytes = body["byteLength"] as? Int {
                    lines.append("\(label)：\(bytes) 字节")
                }
            }
        }
        if let notes = flow["notes"] as? [String], !notes.isEmpty {
            lines += ["", "说明"] + notes
        }
        return lines.joined(separator: "\n")
    }

    private static func appendHeaders(_ title: String, side: [String: Any], into lines: inout [String]) {
        lines += ["", title]
        guard let headers = side["headers"] as? [[String: Any]], !headers.isEmpty else {
            lines.append("（无）")
            return
        }
        for header in headers {
            let disabled = header["disabled"] as? Bool == true ? "[停用] " : ""
            let name = header["name"] as? String ?? ""
            let value = header["value"] as? String ?? ""
            lines.append("\(disabled)\(name): \(value)")
        }
    }
}
