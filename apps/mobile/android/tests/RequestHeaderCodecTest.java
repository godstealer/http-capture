package dev.httpcapture.mobile;

import java.util.List;

public final class RequestHeaderCodecTest {
    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    public static void main(String[] args) {
        String copied = "X-Test: one\nX-Test: two\n# X-Ignored: off";
        List<RequestHeaderCodec.Header> rows = RequestHeaderCodec.parse(copied);
        check(rows.size() == 3 && rows.get(0).value.equals("one")
              && rows.get(1).value.equals("two") && rows.get(2).disabled,
              "复制时应保留重复请求头及停用状态");
        rows = RequestHeaderCodec.parse(RequestHeaderCodec.toggleLine(copied, 2));
        check(!rows.get(2).disabled, "启用按钮未改变发送状态");
        rows = RequestHeaderCodec.parse("GET / HTTP/2\n:authority: example.test\nX-Test: one");
        check(rows.size() == 1 && rows.get(0).name.equals("X-Test"),
              "浏览器请求头粘贴未跳过请求行和伪头");
        rows = RequestHeaderCodec.parseResponse("Content-Type: text/plain\n"
            + "# X-Debug: hidden\nX-Debug: visible");
        check(rows.size() == 3 && rows.get(1).disabled && !rows.get(2).disabled
              && rows.get(1).name.equals(rows.get(2).name),
              "响应头编辑应保留重复字段及停用状态");
        rows = RequestHeaderCodec.parseResponse(RequestHeaderCodec.toggleLine(
            "Content-Type: text/plain\n# X-Debug: hidden", 1));
        check(!rows.get(1).disabled, "响应头启用按钮未改变状态");
        try {
            RequestHeaderCodec.parseResponse(":status: 200");
            throw new AssertionError("响应伪头应拒绝编辑");
        } catch (IllegalArgumentException expected) {
            check(expected.getMessage().contains("第 1 行响应头"), "响应头错误应指出行号和类型");
        }
        try {
            RequestHeaderCodec.parse("X-Bad Name: value");
            throw new AssertionError("非法请求头名称应被拒绝");
        } catch (IllegalArgumentException expected) {
            check(expected.getMessage().contains("第 1 行"), "错误应指出行号");
        }
    }
}
