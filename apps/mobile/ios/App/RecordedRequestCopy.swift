import Foundation

struct ManualRequestDraft: Identifiable {
    let id = UUID()
    var savedID: String? = nil
    var savedName: String? = nil
    var savedCollectionID: String? = nil
    let method: String
    let url: String
    let headers: String
    let body: String
    let bodyMode: ManualBodyMode
    let engine: ManualSendEngine

    init(method: String, url: String, headers: String, body: String,
         bodyMode: ManualBodyMode, engine: ManualSendEngine,
         savedID: String? = nil, savedName: String? = nil, savedCollectionID: String? = nil) {
        self.savedID = savedID
        self.savedName = savedName
        self.savedCollectionID = savedCollectionID
        self.method = method
        self.url = url
        self.headers = headers
        self.body = body
        self.bodyMode = bodyMode
        self.engine = engine
    }

    static let empty = ManualRequestDraft(method: "GET", url: "", headers: "",
                                           body: "", bodyMode: .text, engine: .auto)
}

struct RecordedRequestCopy {
    private static let maxEditableCopyBytes = RequestBodyCodec.maxBodyBytes
    let id: String
    let revision: String
    let byteLength: Int
    let method: String
    let url: String
    let headers: String
    let engine: ManualSendEngine
    private(set) var bytes = Data()

    init(flow: [String: Any]) throws {
        guard let id = flow["id"] as? String,
              let revision = flow["bodyRevision"] as? String,
              let request = flow["request"] as? [String: Any],
              let method = request["method"] as? String,
              let url = request["url"] as? String,
              let references = flow["bodyReferences"] as? [String: Any],
              let reference = references["request"] as? [String: Any],
              let length = (reference["byteLength"] as? NSNumber)?.intValue else {
            throw CopyError.invalid("这条记录缺少请求或正文信息，请重新打开详情")
        }
        guard !method.isEmpty, !url.isEmpty,
              method == method.trimmingCharacters(in: .whitespacesAndNewlines).uppercased(),
              url == url.trimmingCharacters(in: .whitespacesAndNewlines) else {
            throw CopyError.invalid("这条记录的方法或 URL 无法无损复制")
        }
        guard (0...Self.maxEditableCopyBytes).contains(length) else {
            throw CopyError.invalid("请求正文超过 1 MiB，当前 iOS 编辑器不能复制后重发")
        }
        self.id = id
        self.revision = revision
        byteLength = length
        self.method = method
        self.url = url
        guard let engine = ManualSendEngine(rawValue: request["engine"] as? String ?? "auto") else {
            throw CopyError.invalid("原请求的发送引擎未内置于移动版，无法无损复制")
        }
        self.engine = engine
        try EditableRequestMetadata.validate(request)
        guard let rows = request["headers"] as? [[String: Any]] else {
            throw CopyError.invalid("这条记录的请求头不完整")
        }
        var lines: [String] = []
        for row in rows {
            guard let name = row["name"] as? String,
                  let value = row["value"] as? String else {
                throw CopyError.invalid("请求头记录不完整")
            }
            if name.hasPrefix(":") { continue }
            guard name == name.trimmingCharacters(in: .whitespaces),
                  value == value.trimmingCharacters(in: .whitespaces),
                  !name.contains("\r"), !name.contains("\n"),
                  !value.contains("\r"), !value.contains("\n"),
                  RequestHeaderText.isHeaderLine("\(name): \(value)") else {
                throw CopyError.invalid("请求头包含移动编辑器无法表示的字符")
            }
            let prefix = row["disabled"] as? Bool == true ? "# " : ""
            lines.append("\(prefix)\(name): \(value)")
        }
        let headerText = lines.joined(separator: "\n")
        guard headerText.utf8.count <= 16 * 1024 else {
            throw CopyError.invalid("请求头超过 16 KiB，当前 iOS 编辑器不能安全复制后重发")
        }
        headers = headerText
    }

    var offset: Int { bytes.count }

    mutating func append(_ chunk: [String: Any]) throws -> Bool {
        guard let offset = (chunk["offset"] as? NSNumber)?.intValue,
              let total = (chunk["totalBytes"] as? NSNumber)?.intValue,
              let base64 = chunk["bodyBase64"] as? String,
              let part = Data(base64Encoded: base64),
              offset == bytes.count, total == byteLength,
              !part.isEmpty, part.count <= 16 * 1024,
              bytes.count + part.count <= byteLength else {
            throw CopyError.invalid("请求正文分段不完整或已变化，请重新打开详情")
        }
        bytes.append(part)
        let complete = bytes.count == byteLength
        let next = (chunk["nextOffset"] as? NSNumber)?.intValue
        guard complete ? next == nil : next == bytes.count else {
            throw CopyError.invalid("请求正文分页位置不一致，请重新打开详情")
        }
        return complete
    }

    func draft() throws -> ManualRequestDraft {
        guard bytes.count == byteLength else { throw CopyError.invalid("请求正文尚未读完") }
        if bytes.count <= RequestBodyCodec.maxTextBytes,
           let text = ResponseBodyCodec.readableText(bytes) {
            return ManualRequestDraft(method: method, url: url, headers: headers,
                                      body: text, bodyMode: .text, engine: engine)
        }
        return ManualRequestDraft(method: method, url: url, headers: headers,
                                  body: bytes.base64EncodedString(), bodyMode: .base64,
                                  engine: engine)
    }

    enum CopyError: LocalizedError {
        case invalid(String)
        var errorDescription: String? {
            if case .invalid(let reason) = self { return reason }
            return nil
        }
    }
}

enum RequestHeaderText {
    enum HeaderError: LocalizedError {
        case invalid(Int)
        case invalidResponse(Int)
        var errorDescription: String? {
            if case .invalid(let line) = self { return "第 \(line) 行请求头无效，请按 名称: 值 填写" }
            if case .invalidResponse(let line) = self { return "第 \(line) 行响应头无效，请按 名称: 值 填写" }
            return nil
        }
    }

    static func parse(_ text: String) throws -> [[String: Any]] {
        try parse(text, ignoreBrowserRequestMetadata: true)
    }

    static func parseResponse(_ text: String) throws -> [[String: Any]] {
        try parse(text, ignoreBrowserRequestMetadata: false)
    }

    private static func parse(_ text: String, ignoreBrowserRequestMetadata: Bool) throws -> [[String: Any]] {
        var rows: [[String: Any]] = []
        for (index, rawLine) in text.components(separatedBy: .newlines).enumerated() {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.isEmpty { continue }
            let disabled = line.hasPrefix("# ")
            let content = disabled ? String(line.dropFirst(2)) : line
            // Browser copies can include a request line and HTTP/2 pseudo headers.
            let parts = content.split(separator: " ", omittingEmptySubsequences: true)
            if ignoreBrowserRequestMetadata, parts.count >= 3, parts.last?.hasPrefix("HTTP/") == true,
               ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "CONNECT", "TRACE"]
                .contains(String(parts[0])) { continue }
            if ignoreBrowserRequestMetadata, content.hasPrefix(":"), content.dropFirst().contains(":") { continue }
            guard let colon = content.firstIndex(of: ":"), colon != content.startIndex else {
                throw (ignoreBrowserRequestMetadata ? HeaderError.invalid(index + 1)
                    : HeaderError.invalidResponse(index + 1))
            }
            let name = String(content[..<colon]).trimmingCharacters(in: .whitespaces)
            let value = String(content[content.index(after: colon)...]).trimmingCharacters(in: .whitespaces)
            guard !name.isEmpty, name.utf8.allSatisfy({ Self.isTokenByte($0) }),
                  !value.unicodeScalars.contains(where: { $0.value < 0x20 && $0.value != 0x09 }),
                  !value.unicodeScalars.contains(where: { $0.value == 0x7F }) else {
                throw (ignoreBrowserRequestMetadata ? HeaderError.invalid(index + 1)
                    : HeaderError.invalidResponse(index + 1))
            }
            rows.append(["name": name, "value": value, "disabled": disabled])
        }
        return rows
    }

    static func isHeaderLine(_ line: String) -> Bool {
        (try? parse(line).count) == 1
    }

    static func toggle(_ text: String, line index: Int) -> String {
        var lines = text.components(separatedBy: "\n")
        guard lines.indices.contains(index), !lines[index].trimmingCharacters(in: .whitespaces).isEmpty else {
            return text
        }
        let line = lines[index].trimmingCharacters(in: .whitespaces)
        lines[index] = line.hasPrefix("# ") ? String(line.dropFirst(2)) : "# " + line
        return lines.joined(separator: "\n")
    }

    private static func isTokenByte(_ byte: UInt8) -> Bool {
        switch byte {
        case 48...57, 65...90, 97...122: return true
        case 33, 35, 36, 37, 38, 39, 42, 43, 45, 46, 94, 95, 96, 124, 126: return true
        default: return false
        }
    }
}
