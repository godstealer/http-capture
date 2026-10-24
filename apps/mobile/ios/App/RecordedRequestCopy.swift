import Foundation

struct ManualRequestDraft: Identifiable {
    let id = UUID()
    let method: String
    let url: String
    let headers: String
    let body: String
    let bodyMode: ManualBodyMode

    static let empty = ManualRequestDraft(method: "GET", url: "", headers: "",
                                           body: "", bodyMode: .text)
}

struct RecordedRequestCopy {
    private static let maxEditableCopyBytes = 64 * 1024
    let id: String
    let revision: String
    let byteLength: Int
    let method: String
    let url: String
    let headers: String
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
        guard (0...Self.maxEditableCopyBytes).contains(length) else {
            throw CopyError.invalid("请求正文超过 64 KiB，当前 iOS 编辑器不能安全复制后重发")
        }
        self.id = id
        self.revision = revision
        byteLength = length
        self.method = method
        self.url = url
        guard let rows = request["headers"] as? [[String: Any]] else {
            throw CopyError.invalid("这条记录的请求头不完整")
        }
        var lines: [String] = []
        for row in rows where row["disabled"] as? Bool != true {
            guard let name = row["name"] as? String,
                  let value = row["value"] as? String else {
                throw CopyError.invalid("请求头记录不完整")
            }
            if name.hasPrefix(":") { continue }
            guard !name.isEmpty, !name.contains(":"),
                  !name.contains("\r"), !name.contains("\n"),
                  !value.contains("\r"), !value.contains("\n"),
                  name == name.trimmingCharacters(in: .whitespaces),
                  value == value.trimmingCharacters(in: .whitespaces) else {
                throw CopyError.invalid("请求头包含移动编辑器无法表示的字符")
            }
            lines.append("\(name): \(value)")
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
              !part.isEmpty, bytes.count + part.count <= byteLength else {
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
        if let text = String(data: bytes, encoding: .utf8),
           !text.unicodeScalars.contains(where: { scalar in
               scalar.value < 0x20 && scalar.value != 0x09 && scalar.value != 0x0A && scalar.value != 0x0D
           }) {
            return ManualRequestDraft(method: method, url: url, headers: headers,
                                      body: text, bodyMode: .text)
        }
        return ManualRequestDraft(method: method, url: url, headers: headers,
                                  body: bytes.base64EncodedString(), bodyMode: .base64)
    }

    enum CopyError: LocalizedError {
        case invalid(String)
        var errorDescription: String? {
            if case .invalid(let reason) = self { return reason }
            return nil
        }
    }
}
