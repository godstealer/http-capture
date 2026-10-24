package dev.httpcapture.mobile;

import java.util.Arrays;
import java.util.Base64;

/** Run with javac/java on a host JDK; no device or Android test library needed. */
public final class ResponseBodyCodecTest {
    public static void main(String[] args) {
        byte[] binary = {0, (byte) 255, 10, 0};
        String encoded = Base64.getEncoder().encodeToString(binary);
        check(Arrays.equals(binary, ResponseBodyCodec.decodeBase64(" \n" + encoded + "\r\n")),
            "binary body changed after Base64 decode");
        check(ResponseBodyCodec.decodeBase64("").length == 0, "empty body");

        byte[] maximum = new byte[ResponseBodyCodec.MAX_BODY_BYTES];
        maximum[maximum.length - 1] = (byte) 255;
        check(Arrays.equals(maximum, ResponseBodyCodec.decodeBase64(
            Base64.getEncoder().encodeToString(maximum))), "64 KiB boundary");
        reject("!!!!", "invalid characters");
        reject("a", "incomplete encoding");
        reject(Base64.getEncoder().encodeToString(new byte[maximum.length + 1]), "oversize body");
        System.out.println("ResponseBodyCodecTest passed");
    }

    private static void reject(String encoded, String description) {
        try {
            ResponseBodyCodec.decodeBase64(encoded);
            throw new AssertionError(description + " was accepted");
        } catch (IllegalArgumentException expected) { }
    }

    private static void check(boolean condition, String description) {
        if (!condition) throw new AssertionError(description);
    }
}
