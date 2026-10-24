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
        try {
            RequestHeaderCodec.parse("X-Bad Name: value");
            throw new AssertionError("非法请求头名称应被拒绝");
        } catch (IllegalArgumentException expected) {
            check(expected.getMessage().contains("第 1 行"), "错误应指出行号");
        }
    }
}
