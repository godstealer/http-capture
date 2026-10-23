import Foundation

enum ManualBodyMode: String, CaseIterable, Identifiable {
    case text = "文本"
    case json = "JSON"
    case form = "URL 表单"
    case file = "文件"
    case multipart = "Multipart 表单"
    case base64 = "Base64"

    var id: String { rawValue }
}

struct PreparedRequestBody {
    let bytes: Data
    let contentType: String?
    let replaceContentType: Bool
}

enum RequestBodyCodec {
    // Base64 and JSON framing must also fit in a 128 KiB provider message.
    static let maxBodyBytes = 64 * 1024

    static func compose(mode: ManualBodyMode, text: String, file: Data?,
                        fileName: String?, mimeType: String?) throws -> PreparedRequestBody {
        let maxInputBytes = mode == .base64 ? 96 * 1024 : maxBodyBytes
        guard text.utf8.count <= maxInputBytes else {
            throw BodyError.invalid(mode == .base64 ? "Base64 输入最多 96 KiB" : "文本输入最多 64 KiB")
        }
        let bytes: Data
        let contentType: String?
        var replace = false
        switch mode {
        case .text:
            bytes = Data(text.utf8)
            contentType = nil
        case .json:
            if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                guard let data = text.data(using: .utf8),
                      (try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed])) != nil else {
                    throw BodyError.invalid("正文不是有效的 JSON")
                }
            }
            bytes = Data(text.utf8)
            contentType = "application/json; charset=utf-8"
        case .form:
            let pairs = try fields(text)
            bytes = Data(pairs.map { "\(urlEncode($0.0))=\(urlEncode($0.1))" }.joined(separator: "&").utf8)
            contentType = "application/x-www-form-urlencoded"
        case .file:
            guard let file else { throw BodyError.invalid("请先选择要发送的文件") }
            bytes = file
            contentType = safeMimeType(mimeType)
        case .multipart:
            let boundary = "----HttpCapture" + UUID().uuidString.replacingOccurrences(of: "-", with: "")
            var body = Data()
            for (name, value) in try fields(text) {
                try append("--\(boundary)\r\nContent-Disposition: form-data; name=\"\(safeToken(name))\"\r\n\r\n\(value)\r\n", to: &body)
            }
            if let file {
                try append("--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"\(safeToken(fileName ?? "attachment.bin"))\"\r\nContent-Type: \(safeMimeType(mimeType))\r\n\r\n", to: &body)
                guard body.count + file.count <= maxBodyBytes else { throw BodyError.invalid("请求正文最多 64 KiB") }
                body.append(file)
                try append("\r\n", to: &body)
            }
            try append("--\(boundary)--\r\n", to: &body)
            bytes = body
            contentType = "multipart/form-data; boundary=\(boundary)"
            replace = true
        case .base64:
            let compact = text.filter { !$0.isWhitespace }
            guard let decoded = Data(base64Encoded: compact) else { throw BodyError.invalid("Base64 正文无效") }
            bytes = decoded
            contentType = nil
        }
        guard bytes.count <= maxBodyBytes else { throw BodyError.invalid("请求正文最多 64 KiB") }
        return PreparedRequestBody(bytes: bytes, contentType: contentType, replaceContentType: replace)
    }

    private static func fields(_ text: String) throws -> [(String, String)] {
        try text.components(separatedBy: .newlines).filter { !$0.isEmpty }.map { line in
            guard let separator = line.firstIndex(of: "="), separator != line.startIndex else {
                throw BodyError.invalid("表单每行须为 名称=值")
            }
            return (String(line[..<separator]), String(line[line.index(after: separator)...]))
        }
    }

    private static func urlEncode(_ value: String) -> String {
        let safe = Array("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._*").map { $0.asciiValue! }
        return value.utf8.map { byte in
            if byte == 32 { return "+" }
            if safe.contains(byte) { return String(UnicodeScalar(Int(byte))!) }
            return String(format: "%%%02X", byte)
        }.joined()
    }

    private static func safeToken(_ value: String) throws -> String {
        let cleaned = value.map { "\"\r\n\\/".contains($0) ? "_" : String($0) }.joined()
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleaned.isEmpty, cleaned.count <= 128 else { throw BodyError.invalid("Multipart 字段名或文件名无效") }
        return cleaned
    }

    private static func safeMimeType(_ value: String?) -> String {
        guard let value, value.split(separator: "/", omittingEmptySubsequences: false).count == 2,
              !value.hasPrefix("/"), !value.hasSuffix("/"),
              value.utf8.allSatisfy({ byte in
                  (65...90).contains(byte) || (97...122).contains(byte) || (48...57).contains(byte)
                  || "!#$&^_.+-/".utf8.contains(byte)
              }) else { return "application/octet-stream" }
        return value
    }

    private static func append(_ value: String, to body: inout Data) throws {
        guard body.count + value.utf8.count <= maxBodyBytes else { throw BodyError.invalid("请求正文最多 64 KiB") }
        body.append(contentsOf: value.utf8)
    }

    enum BodyError: LocalizedError {
        case invalid(String)
        var errorDescription: String? {
            if case .invalid(let reason) = self { return reason }
            return nil
        }
    }
}
