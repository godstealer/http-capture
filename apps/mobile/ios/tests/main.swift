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
                                                 fileName: "test\".bin", mimeType: "bad\r\ntype",
                                                 multipartFileField: "upload")
    let boundary = String(multipart.contentType!.split(separator: "=").last!)
    expect(multipart.replaceContentType, "Multipart 必须更新旧 Content-Type")
    expect(multipart.bytes.starts(with: Data("--\(boundary)\r\n".utf8)), "Multipart 起始边界错误")
    expect(multipart.bytes.range(of: Data("name=\"upload\"; filename=\"test_.bin\"".utf8)) != nil,
           "文件字段名或文件名没有正确编码")
    expect(multipart.bytes.range(of: Data("Content-Type: application/octet-stream".utf8)) != nil,
           "非法 MIME 未回退")
    expect(multipart.bytes.range(of: binary) != nil, "文件原始字节丢失")
    expect(Data(multipart.bytes.suffix(Data("--\(boundary)--\r\n".utf8).count)) == Data("--\(boundary)--\r\n".utf8),
           "Multipart 结束边界错误")
    do {
        _ = try RequestBodyCodec.compose(mode: .multipart, text: "", file: binary,
                                         fileName: "test.bin", mimeType: "text/plain",
                                         multipartFileField: "")
        preconditionFailure("空文件字段名应被拒绝")
    } catch RequestBodyCodec.BodyError.invalid(_) { }

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
        "request": ["method": "POST", "url": "https://example.test/upload", "engine": "h2", "headers": [
            ["name": ":authority", "value": "example.test"],
            ["name": "X-Test", "value": "one"],
            ["name": "X-Test", "value": "two"],
            ["name": "X-Ignored", "value": "off", "disabled": true]
        ]],
        "bodyReferences": ["request": ["byteLength": 3]]
    ]
    let detail = FlowDetailFormatter.format(flow)
    expect(detail.contains("POST https://example.test/upload"), "请求摘要没有显示方法与 URL")
    expect(detail.range(of: "X-Test: one\nX-Test: two") != nil,
           "请求头顺序或重复项在详情中丢失")
    expect(detail.contains("[停用] X-Ignored: off"), "停用请求头未标明")
    expect(detail.contains("请求：3 字节"), "详情没有显示正文大小")
    var copy = try RecordedRequestCopy(flow: flow)
    expect(copy.headers == "X-Test: one\nX-Test: two\n# X-Ignored: off",
           "请求头顺序、重复项或停用状态丢失")
    let copiedHeaders = try RequestHeaderText.parse(copy.headers)
    expect(copiedHeaders.count == 3 && copiedHeaders[2]["disabled"] as? Bool == true,
           "停用请求头未传入重放草稿")
    let enabledText = RequestHeaderText.toggle(copy.headers, line: 2)
    let enabledHeaders = try RequestHeaderText.parse(enabledText)
    expect(enabledHeaders[2]["disabled"] as? Bool == false,
           "请求头启用按钮未改变发送状态")
    let browserHeaders = try RequestHeaderText.parse("GET /upload HTTP/2\n:authority: example.test\nX-Test: one\nX-Test: two")
    expect(browserHeaders.count == 2 && browserHeaders[0]["value"] as? String == "one",
           "浏览器请求头粘贴未保留普通请求头顺序")
    do {
        _ = try RequestHeaderText.parse("X-Bad Name: value")
        preconditionFailure("非法请求头名称应被拒绝")
    } catch RequestHeaderText.HeaderError.invalid(_) { }
    let responseHeaders = try RequestHeaderText.parseResponse(
        "Content-Type: text/plain\n# X-Debug: hidden\nX-Debug: visible")
    expect(responseHeaders.count == 3 && responseHeaders[1]["disabled"] as? Bool == true
           && responseHeaders[2]["disabled"] as? Bool == false,
           "响应头顺序、重复项或停用状态丢失")
    let enabledResponse = try RequestHeaderText.parseResponse(RequestHeaderText.toggle(
        "Content-Type: text/plain\n# X-Debug: hidden", line: 1))
    expect(enabledResponse[1]["disabled"] as? Bool == false, "响应头启用状态未更新")
    do {
        _ = try RequestHeaderText.parseResponse(":status: 200")
        preconditionFailure("响应伪头应拒绝编辑")
    } catch RequestHeaderText.HeaderError.invalidResponse(let line) {
        expect(line == 1, "响应头错误应包含行号")
    }
    let firstComplete = try copy.append(["offset": 0, "totalBytes": 3,
                                         "bodyBase64": "AP8=", "nextOffset": 2])
    expect(!firstComplete, "第一页提前结束")
    let lastComplete = try copy.append(["offset": 2, "totalBytes": 3,
                                        "bodyBase64": "Cg==", "nextOffset": NSNull()])
    expect(lastComplete, "末页未完成")
    let replay = try copy.draft()
    expect(replay.method == "POST" && replay.engine == .h2
           && replay.bodyMode == .base64 && replay.body == "AP8K",
           "二进制请求复制后字节改变")
    var unsupported = flow
    var unsupportedRequest = unsupported["request"] as! [String: Any]
    unsupportedRequest["engine"] = "wreq"
    unsupported["request"] = unsupportedRequest
    do {
        _ = try RecordedRequestCopy(flow: unsupported)
        preconditionFailure("未内置的发送引擎不能静默降级")
    } catch RecordedRequestCopy.CopyError.invalid(_) { }
    for (key, value) in [
        ("pseudoHeaders", [["name": ":authority", "value": "other.test"]] as Any),
        ("upstreamProfileId", "proxy-profile" as Any),
        ("scripts", ["enabled": true] as Any),
        ("tls", ["preset": "chrome"] as Any)
    ] {
        var changed = flow
        var request = changed["request"] as! [String: Any]
        request[key] = value
        changed["request"] = request
        do {
            _ = try RecordedRequestCopy(flow: changed)
            preconditionFailure("复制请求不能静默丢失 \(key)")
        } catch EditableRequestMetadata.CopyError.invalid(_) { }
    }
    let largestBinary = Data(repeating: 255, count: RequestBodyCodec.maxBodyBytes)
    let largestEncoded = try RequestBodyCodec.compose(mode: .base64,
        text: largestBinary.base64EncodedString(), file: nil, fileName: nil, mimeType: nil)
    expect(largestEncoded.bytes == largestBinary, "1 MiB 二进制正文无法通过 Base64 编辑器重发")
    var largeFlow = flow
    largeFlow["bodyReferences"] = ["request": ["byteLength": largestBinary.count]]
    var largeCopy = try RecordedRequestCopy(flow: largeFlow)
    for offset in stride(from: 0, to: largestBinary.count, by: 16 * 1024) {
        let end = min(offset + 16 * 1024, largestBinary.count)
        let chunk = largestBinary.subdata(in: offset..<end)
        let nextOffset: Any = end == largestBinary.count ? NSNull() as Any : end as Any
        _ = try largeCopy.append(["offset": offset, "totalBytes": largestBinary.count,
                                  "bodyBase64": chunk.base64EncodedString(),
                                  "nextOffset": nextOffset])
    }
    let largeDraft = try largeCopy.draft()
    expect(largeDraft.bodyMode == .base64
           && Data(base64Encoded: largeDraft.body) == largestBinary,
           "1 MiB 抓包正文复制后字节改变")
    let overTextLimit = Data(repeating: 65, count: RequestBodyCodec.maxTextBytes + 1)
    largeFlow["bodyReferences"] = ["request": ["byteLength": overTextLimit.count]]
    var textCopy = try RecordedRequestCopy(flow: largeFlow)
    for offset in stride(from: 0, to: overTextLimit.count, by: 16 * 1024) {
        let end = min(offset + 16 * 1024, overTextLimit.count)
        let chunk = overTextLimit.subdata(in: offset..<end)
        let nextOffset: Any = end == overTextLimit.count ? NSNull() as Any : end as Any
        _ = try textCopy.append(["offset": offset, "totalBytes": overTextLimit.count,
                                 "bodyBase64": chunk.base64EncodedString(),
                                 "nextOffset": nextOffset])
    }
    let textDraft = try textCopy.draft()
    expect(textDraft.bodyMode == .base64,
           "超过 64 KiB 的可读正文应切换到 Base64 编辑器")
    largeFlow["bodyReferences"] = ["request": ["byteLength": largestBinary.count + 1]]
    do {
        _ = try RecordedRequestCopy(flow: largeFlow)
        preconditionFailure("超过 1 MiB 的抓包正文不应进入编辑器")
    } catch RecordedRequestCopy.CopyError.invalid(_) { }
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
    for (key, value) in [("method", " POST"), ("method", "post"),
                         ("url", "https://example.test/ ")] {
        var malformed = flow
        var malformedRequest = malformed["request"] as! [String: Any]
        malformedRequest[key] = value
        malformed["request"] = malformedRequest
        do {
            _ = try RecordedRequestCopy(flow: malformed)
            preconditionFailure("方法或 URL 含边界空格时不能静默改写")
        } catch RecordedRequestCopy.CopyError.invalid(_) { }
    }
    var malformedHeader = flow
    var malformedHeaderRequest = malformedHeader["request"] as! [String: Any]
    malformedHeaderRequest["headers"] = [["name": "X-Bad Name", "value": "value"]]
    malformedHeader["request"] = malformedHeaderRequest
    do {
        _ = try RecordedRequestCopy(flow: malformedHeader)
        preconditionFailure("移动编辑器无法发送的头名称应在复制阶段拒绝")
    } catch RecordedRequestCopy.CopyError.invalid(_) { }
    malformedHeaderRequest["headers"] = [["name": "X-Test", "value": "one\nGET / HTTP/1.1"]]
    malformedHeader["request"] = malformedHeaderRequest
    do {
        _ = try RecordedRequestCopy(flow: malformedHeader)
        preconditionFailure("请求头值中的换行不能被当作浏览器请求行忽略")
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
    let responseBinary = Data([0, 255, 10, 0])
    let decodedResponse = try ResponseBodyCodec.decodeBase64(
        " \n" + responseBinary.base64EncodedString() + "\r")
    expect(decodedResponse == responseBinary, "二进制响应正文被 Base64 解码改动")
    expect(ResponseBodyCodec.readableText(Data("中文\ntext".utf8)) == "中文\ntext",
           "普通 UTF-8 响应应使用文本编辑器")
    expect(ResponseBodyCodec.readableText(Data([65, 0, 66])) == nil,
           "含 NUL 的响应应使用 Base64 编辑器")
    expect(ResponseBodyCodec.readableText(Data([255])) == nil,
           "无效 UTF-8 响应应使用 Base64 编辑器")
    let maximumResponse = Data(repeating: 255, count: ResponseBodyCodec.maxBodyBytes)
    let decodedMaximum = try ResponseBodyCodec.decodeBase64(maximumResponse.base64EncodedString())
    expect(decodedMaximum == maximumResponse, "64 KiB 响应正文边界错误")
    for invalidResponse in ["!!!!", "a", Data(count: ResponseBodyCodec.maxBodyBytes + 1)
        .base64EncodedString()] {
        do {
            _ = try ResponseBodyCodec.decodeBase64(invalidResponse)
            preconditionFailure("无效或超限响应正文应被拒绝")
        } catch ResponseBodyCodec.BodyError.invalid(_) { }
    }
    let savedBody = Data(repeating: 255, count: 12 * 1024 + 1)
    let savedValue: [String: Any] = [
        "request": ["id": "saved-1", "name": "保存的请求", "collectionId": "collection-1", "draft": [
            "method": "POST", "url": "https://example.test/saved", "engine": "h2",
            "headers": [["name": "X-Order", "value": "first"],
                        ["name": "x-order", "value": "second"],
                        ["name": "X-Disabled", "value": "off", "disabled": true]],
            "bodyBase64": ""]],
        "totalBytes": NSNumber(value: savedBody.count), "readToken": NSNumber(value: UInt64(7))
    ]
    var savedCopy = try SavedRequestCopy(savedValue)
    expect(savedCopy.headers == "X-Order: first\nx-order: second\n# X-Disabled: off",
           "已保存请求的请求头顺序、重复项或停用状态丢失")
    let firstSaved = try savedCopy.append(["offset": 0, "totalBytes": savedBody.count,
        "bodyBase64": Data(savedBody.prefix(12 * 1024)).base64EncodedString(),
        "nextOffset": 12 * 1024])
    expect(!firstSaved, "已保存请求第一页正文提前结束")
    let lastSaved = try savedCopy.append(["offset": 12 * 1024, "totalBytes": savedBody.count,
        "bodyBase64": Data(savedBody.suffix(1)).base64EncodedString(), "nextOffset": NSNull()])
    let editableSaved = try savedCopy.draft()
    expect(lastSaved && editableSaved.savedID == "saved-1" && editableSaved.bodyMode == .base64
           && editableSaved.savedCollectionID == "collection-1"
           && Data(base64Encoded: editableSaved.body) == savedBody,
           "已保存请求的分段二进制正文不能无损编辑")
    var lossySaved = savedValue
    var lossyRequest = lossySaved["request"] as! [String: Any]
    var lossyDraft = lossyRequest["draft"] as! [String: Any]
    lossyDraft["headers"] = [["name": "X-Test", "value": " leading"]]
    lossyRequest["draft"] = lossyDraft
    lossySaved["request"] = lossyRequest
    do {
        _ = try SavedRequestCopy(lossySaved)
        preconditionFailure("已保存请求头的前导空格不能被静默删除")
    } catch SavedRequestCopy.CopyError.invalid(_) { }
    for (key, target) in [("method", "post"), ("url", "https://example.test/saved ")] {
        var changedRequest = savedValue["request"] as! [String: Any]
        var changedDraft = changedRequest["draft"] as! [String: Any]
        changedDraft[key] = target
        changedRequest["draft"] = changedDraft
        do {
            _ = try SavedRequestCopy(["request": changedRequest,
                "totalBytes": NSNumber(value: savedBody.count),
                "readToken": NSNumber(value: UInt64(7))])
            preconditionFailure("已保存请求的方法或 URL 不能被静默改写")
        } catch SavedRequestCopy.CopyError.invalid(_) { }
    }
    print("iOS request and response body codecs: PASS")
} catch {
    fatalError(error.localizedDescription)
}
