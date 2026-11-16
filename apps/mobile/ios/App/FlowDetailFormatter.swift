import Foundation

enum RecordedBodyCapture {
    static func isRecorded(_ flow: [String: Any], variant: String) -> Bool {
        guard ["request", "response", "originalRequest", "originalResponse"].contains(variant) else { return false }
        if let raw = flow["bodyCapture"] {
            guard let capture = raw as? [String: Any] else { return false }
            if let state = capture[variant] { return state as? String == "recorded" }
        }
        return ((flow["trafficOrigin"] as? [String: Any])?["entry"] as? String) != "android-lan"
    }

    static func hasBody(_ flow: [String: Any], variant: String) -> Bool {
        guard isRecorded(flow, variant: variant),
              let references = flow["bodyReferences"] as? [String: Any] else { return false }
        return references[variant] is [String: Any]
    }
}

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
        let bodies = flow["bodyReferences"] as? [String: Any] ?? [:]
        lines += ["", "正文"]
        for (key, label) in [("request", "请求"), ("response", "响应")] {
            guard flow[key] is [String: Any] else { continue }
            if !RecordedBodyCapture.isRecorded(flow, variant: key) {
                lines.append("\(label)：正文未记录")
            } else if let body = bodies[key] as? [String: Any],
                      let bytes = body["byteLength"] as? Int {
                lines.append("\(label)：\(bytes) 字节")
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
