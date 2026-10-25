package dev.httpcapture.mobile;

import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;

/** Run with javac/java on a host JDK; no device or Android test library needed. */
public final class RequestBodyCodecTest {
    public static void main(String[] args) throws Exception {
        String form = new String(RequestBodyCodec.urlForm("name=张 三\ncode=1+2"), StandardCharsets.US_ASCII);
        check(form.equals("name=%E5%BC%A0+%E4%B8%89&code=1%2B2"), "URL form encoding");

        byte[] file = {0, 10, (byte) 255};
        byte[] actual = RequestBodyCodec.multipart("city=北九州", "----Test",
            "upload", "a\".bin", "text/plain", file);
        ByteArrayOutputStream expected = new ByteArrayOutputStream();
        expected.write(("------Test\r\nContent-Disposition: form-data; name=\"city\"\r\n\r\n"
            + "北九州\r\n------Test\r\nContent-Disposition: form-data; name=\"upload\"; "
            + "filename=\"a_.bin\"\r\nContent-Type: text/plain\r\n\r\n")
            .getBytes(StandardCharsets.UTF_8));
        expected.write(file);
        expected.write("\r\n------Test--\r\n".getBytes(StandardCharsets.UTF_8));
        check(Arrays.equals(actual, expected.toByteArray()), "multipart wire bytes");
        check(RequestBodyCodec.safeMimeType("text/plain\r\nX-Test: bad")
            .equals("application/octet-stream"), "unsafe MIME fallback");
        try {
            RequestBodyCodec.multipart("", "----Test", "file", "big.bin", "application/octet-stream",
                new byte[RequestBodyCodec.MAX_BODY_BYTES]);
            throw new AssertionError("oversize multipart accepted");
        } catch (IllegalArgumentException expectedFailure) {
            check(expectedFailure.getMessage().contains("1 MiB"), "size error");
        }
        try {
            RequestBodyCodec.multipart("", "----Test", "", "a.bin", "text/plain", file);
            throw new AssertionError("empty file field accepted");
        } catch (IllegalArgumentException expectedFailure) {
            check(expectedFailure.getMessage().contains("字段名"), "file field error");
        }
        for (String invalidName : new String[] {"upload\tother", "bad\u0000name"}) {
            try {
                RequestBodyCodec.multipart("", "----Test", invalidName, "a.bin", "text/plain", file);
                throw new AssertionError("control character in multipart field accepted");
            } catch (IllegalArgumentException expectedFailure) {
                check(expectedFailure.getMessage().contains("控制字符"), "control character error");
            }
        }
        try {
            RequestBodyCodec.multipart("", "----Test", "file", "a\rb.bin", "text/plain", file);
            throw new AssertionError("control character in multipart filename accepted");
        } catch (IllegalArgumentException expectedFailure) {
            check(expectedFailure.getMessage().contains("控制字符"), "filename control character error");
        }
        System.out.println("RequestBodyCodecTest passed");
    }

    private static void check(boolean condition, String description) {
        if (!condition) throw new AssertionError(description);
    }
}
