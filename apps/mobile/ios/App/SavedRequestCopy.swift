import Foundation

struct SavedRequestSummary: Identifiable {
    let id: String
    let name: String
    let collectionName: String
    let method: String
    let url: String

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String else { return nil }
        self.id = id
        name = value["name"] as? String ?? ""
        collectionName = value["collectionName"] as? String ?? ""
        method = value["method"] as? String ?? ""
        url = value["url"] as? String ?? ""
    }
}

struct SavedRequestCopy {
    let savedID: String
    let savedName: String
    let method: String
    let url: String
    let headers: String
    let engine: ManualSendEngine
    let totalBytes: Int
    let readToken: UInt64
    private(set) var bytes = Data()

    init(_ value: [String: Any]) throws {
        guard let request = value["request"] as? [String: Any],
              let draft = request["draft"] as? [String: Any],
              let id = request["id"] as? String,
              let name = request["name"] as? String,
              let method = draft["method"] as? String,
              let url = draft["url"] as? String,
              let engine = ManualSendEngine(rawValue: draft["engine"] as? String ?? "auto"),
              let count = (value["totalBytes"] as? NSNumber)?.intValue,
              let token = (value["readToken"] as? NSNumber)?.uint64Value,
              (0...RequestBodyCodec.maxBodyBytes).contains(count) else {
            throw CopyError.invalid("保存的请求信息不完整或正文超过 1 MiB")
        }
        guard method == method.trimmingCharacters(in: .whitespacesAndNewlines).uppercased(),
              url == url.trimmingCharacters(in: .whitespacesAndNewlines) else {
            throw CopyError.invalid("此请求的方法或 URL 会被移动编辑器改写，暂不能无损修改")
        }
        if request["grpcDraft"] is [String: Any]
            || (request["requestMode"] as? String)?.lowercased() == "grpc" {
            throw CopyError.invalid("gRPC 请求暂不能在移动 HTTP 编辑器中打开")
        }
        if let pseudo = draft["pseudoHeaders"] as? [Any], !pseudo.isEmpty {
            throw CopyError.invalid("HTTP/2 伪头暂不能在移动编辑器中无损修改")
        }
        if let upstream = draft["upstreamProfileId"], !(upstream is NSNull) {
            throw CopyError.invalid("含上游代理配置的请求暂不能无损修改")
        }
        if let scripts = draft["scripts"] as? [String: Any] {
            let modules = scripts["modules"] as? [String: Any] ?? [:]
            let active = scripts["enabled"] as? Bool == true
                || !(scripts["before"] as? String ?? "").isEmpty
                || !(scripts["after"] as? String ?? "").isEmpty
                || !(scripts["variables"] as? [String: Any] ?? [:]).isEmpty
                || modules["encoding"] as? Bool == false
                || modules["crypto"] as? Bool == false
                || modules["utils"] as? Bool == false
            if active { throw CopyError.invalid("含脚本配置的请求暂不能无损修改") }
        }
        if let tls = draft["tls"] as? [String: Any] {
            let custom = ["clientHelloHex", "browserVersion", "version", "cipherList",
                          "sigalgsList", "curvesList", "grease", "permuteExtensions"]
                .contains { key in tls[key] != nil && !(tls[key] is NSNull) }
            if custom || (tls["preset"] as? String ?? "native") != "native" {
                throw CopyError.invalid("含自定义 TLS 配置的请求暂不能无损修改")
            }
        }
        guard let rows = draft["headers"] as? [[String: Any]] else {
            throw CopyError.invalid("保存的请求头不完整")
        }
        var lines: [String] = []
        for row in rows {
            guard let field = row["name"] as? String,
                  let content = row["value"] as? String,
                  field == field.trimmingCharacters(in: .whitespaces),
                  content == content.trimmingCharacters(in: .whitespaces),
                  !field.contains("\r"), !field.contains("\n"),
                  !content.contains("\r"), !content.contains("\n") else {
                throw CopyError.invalid("请求头包含移动编辑器无法表示的字符")
            }
            let line = (row["disabled"] as? Bool == true ? "# " : "") + field + ": " + content
            guard let parsed = try? RequestHeaderText.parse(line), parsed.count == 1,
                  parsed[0]["name"] as? String == field,
                  parsed[0]["value"] as? String == content,
                  parsed[0]["disabled"] as? Bool == (row["disabled"] as? Bool == true) else {
                throw CopyError.invalid("请求头不能无损还原到编辑器")
            }
            lines.append(line)
        }
        let headerText = lines.joined(separator: "\n")
        guard headerText.utf8.count <= 16 * 1024 else {
            throw CopyError.invalid("请求头超过移动编辑器的 16 KiB 限制")
        }
        savedID = id
        savedName = name
        self.method = method
        self.url = url
        self.headers = headerText
        self.engine = engine
        totalBytes = count
        readToken = token
    }

    var offset: Int { bytes.count }

    mutating func append(_ chunk: [String: Any]) throws -> Bool {
        guard let offset = (chunk["offset"] as? NSNumber)?.intValue,
              let total = (chunk["totalBytes"] as? NSNumber)?.intValue,
              let encoded = chunk["bodyBase64"] as? String,
              let part = Data(base64Encoded: encoded),
              offset == bytes.count, total == totalBytes,
              !part.isEmpty, part.count <= 12 * 1024,
              bytes.count + part.count <= totalBytes else {
            throw CopyError.invalid("保存的请求正文分段不完整，请重新打开")
        }
        bytes.append(part)
        let complete = bytes.count == totalBytes
        let next = (chunk["nextOffset"] as? NSNumber)?.intValue
        guard complete ? next == nil : next == bytes.count else {
            throw CopyError.invalid("保存的请求正文分页位置不一致，请重新打开")
        }
        return complete
    }

    func draft() throws -> ManualRequestDraft {
        guard bytes.count == totalBytes else { throw CopyError.invalid("请求正文尚未读完") }
        if bytes.count <= RequestBodyCodec.maxTextBytes,
           let text = ResponseBodyCodec.readableText(bytes) {
            return ManualRequestDraft(method: method, url: url, headers: headers,
                                      body: text, bodyMode: .text, engine: engine,
                                      savedID: savedID, savedName: savedName)
        }
        return ManualRequestDraft(method: method, url: url, headers: headers,
                                  body: bytes.base64EncodedString(), bodyMode: .base64,
                                  engine: engine, savedID: savedID, savedName: savedName)
    }

    enum CopyError: LocalizedError {
        case invalid(String)
        var errorDescription: String? {
            if case .invalid(let message) = self { return message }
            return nil
        }
    }
}
