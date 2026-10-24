import Foundation

func expect(_ condition: @autoclosure () -> Bool, _ reason: String) {
    precondition(condition(), reason)
}

do {
    let form = try RequestBodyCodec.compose(mode: .form, text: "城市=台北 桃园\nx=a+b", file: nil,
                                            fileName: nil, mimeType: nil)
    expect(String(data: form.bytes, encoding: .utf8) ==
           "%E5%9F%8E%E5%B8%82=%E5%8F%B0%E5%8C%97+%E6%A1%83%E5%9B%AD&x=a%2Bb",
           "URL 表单编码错误")
    expect(form.contentType == "application/x-www-form-urlencoded", "URL 表单 Content-Type 错误")

    let json = try RequestBodyCodec.compose(mode: .json, text: "{\"ok\":true}", file: nil,
                                            fileName: nil, mimeType: nil)
    expect(json.bytes == Data("{\"ok\":true}".utf8), "JSON 正文被改动")
    expect(json.contentType == "application/json; charset=utf-8", "JSON Content-Type 错误")
    do {
        _ = try RequestBodyCodec.compose(mode: .json, text: "{broken", file: nil,
                                         fileName: nil, mimeType: nil)
        preconditionFailure("无效 JSON 应被拒绝")
    } catch RequestBodyCodec.BodyError.invalid(_) { }

    let binary = Data([0, 255, 10])
    let multipart = try RequestBodyCodec.compose(mode: .multipart, text: "city=台北", file: binary,
                                                 fileName: "test\".bin", mimeType: "bad\r\ntype")
    let boundary = String(multipart.contentType!.split(separator: "=").last!)
    expect(multipart.replaceContentType, "Multipart 必须更新旧 Content-Type")
    expect(multipart.bytes.starts(with: Data("--\(boundary)\r\n".utf8)), "Multipart 起始边界错误")
    expect(multipart.bytes.range(of: Data("name=\"file\"; filename=\"test_.bin\"".utf8)) != nil,
           "文件名没有安全编码")
    expect(multipart.bytes.range(of: Data("Content-Type: application/octet-stream".utf8)) != nil,
           "非法 MIME 未回退")
    expect(multipart.bytes.range(of: binary) != nil, "文件原始字节丢失")
    expect(Data(multipart.bytes.suffix(Data("--\(boundary)--\r\n".utf8).count)) == Data("--\(boundary)--\r\n".utf8),
           "Multipart 结束边界错误")

    let raw = try RequestBodyCodec.compose(mode: .base64, text: "AP8K\n", file: nil,
                                           fileName: nil, mimeType: nil)
    expect(raw.bytes == binary, "Base64 解码错误")
    do {
        _ = try RequestBodyCodec.compose(mode: .file, text: "", file: Data(count: 1024 * 1024 + 1),
                                         fileName: "large", mimeType: nil)
        preconditionFailure("超大文件应被拒绝")
    } catch RequestBodyCodec.BodyError.invalid(_) { }

    let flow: [String: Any] = [
        "id": "recorded", "bodyRevision": "run:1",
        "request": ["method": "POST", "url": "https://example.test/upload", "headers": [
            ["name": ":authority", "value": "example.test"],
            ["name": "X-Test", "value": "one"],
            ["name": "X-Test", "value": "two"],
            ["name": "X-Ignored", "value": "off", "disabled": true]
        ]],
        "bodyReferences": ["request": ["byteLength": 3]]
    ]
    var copy = try RecordedRequestCopy(flow: flow)
    expect(copy.headers == "X-Test: one\nX-Test: two", "请求头顺序或重复项丢失")
    let firstComplete = try copy.append(["offset": 0, "totalBytes": 3,
                                         "bodyBase64": "AP8=", "nextOffset": 2])
    expect(!firstComplete, "第一页提前结束")
    let lastComplete = try copy.append(["offset": 2, "totalBytes": 3,
                                        "bodyBase64": "Cg==", "nextOffset": NSNull()])
    expect(lastComplete, "末页未完成")
    let replay = try copy.draft()
    expect(replay.method == "POST" && replay.bodyMode == .base64 && replay.body == "AP8K",
           "二进制请求复制后字节改变")
    let largestBinary = Data(repeating: 255, count: RequestBodyCodec.maxBodyBytes)
    let largestEncoded = try RequestBodyCodec.compose(mode: .base64,
        text: largestBinary.base64EncodedString(), file: nil, fileName: nil, mimeType: nil)
    expect(largestEncoded.bytes == largestBinary, "1 MiB 二进制正文无法通过 Base64 编辑器重发")
    let fileURL = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: fileURL) }
    try largestBinary.write(to: fileURL)
    let fileHandle = try FileHandle(forReadingFrom: fileURL)
    let loadedFile = try RequestBodyCodec.readFile(fileHandle)
    expect(loadedFile == largestBinary, "文件读取丢失原始字节")
    try fileHandle.close()
    try Data(count: RequestBodyCodec.maxBodyBytes + 1).write(to: fileURL)
    let oversizedHandle = try FileHandle(forReadingFrom: fileURL)
    defer { try? oversizedHandle.close() }
    do {
        _ = try RequestBodyCodec.readFile(oversizedHandle)
        preconditionFailure("超大文件应在读取时被拒绝")
    } catch RequestBodyCodec.BodyError.invalid(_) { }
    do {
        var invalid = try RecordedRequestCopy(flow: flow)
        _ = try invalid.append(["offset": 1, "totalBytes": 3, "bodyBase64": "AP8=", "nextOffset": 3])
        preconditionFailure("不连续的正文分段应被拒绝")
    } catch RecordedRequestCopy.CopyError.invalid(_) { }
    var spaced = flow
    var request = spaced["request"] as! [String: Any]
    request["headers"] = [["name": "X-Test", "value": " value"]]
    spaced["request"] = request
    do {
        _ = try RecordedRequestCopy(flow: spaced)
        preconditionFailure("无法保留前导空格的请求头不能静默改写")
    } catch RecordedRequestCopy.CopyError.invalid(_) { }
    let controlFlow: [String: Any] = [
        "id": "control", "bodyRevision": "run:1",
        "request": ["method": "POST", "url": "https://example.test/", "headers": []],
        "bodyReferences": ["request": ["byteLength": 2]]
    ]
    var control = try RecordedRequestCopy(flow: controlFlow)
    let controlComplete = try control.append(["offset": 0, "totalBytes": 2,
                                              "bodyBase64": "AEE=", "nextOffset": NSNull()])
    let controlDraft = try control.draft()
    expect(controlComplete && controlDraft.bodyMode == .base64,
           "含控制字节的 UTF-8 正文应通过 Base64 编辑")
    print("iOS request body codec: PASS")
} catch {
    fatalError(error.localizedDescription)
}
