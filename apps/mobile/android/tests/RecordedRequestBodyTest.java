package dev.httpcapture.mobile;

import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.Base64;

/** Host JDK regression for copying captured request bodies without changing bytes. */
public final class RecordedRequestBodyTest {
    public static void main(String[] args) {
        byte[] text = "中文\ntext".getBytes(StandardCharsets.UTF_8);
        RecordedRequestBody readable = RecordedRequestBody.from(text);
        check(readable.mode == 0 && Arrays.equals(text,
                readable.value.getBytes(StandardCharsets.UTF_8)), "UTF-8 text bytes changed");

        byte[] binary = {0, (byte) 255, 65, 10};
        RecordedRequestBody encoded = RecordedRequestBody.from(binary);
        check(encoded.mode == 5 && Arrays.equals(binary,
                Base64.getDecoder().decode(encoded.value)), "binary request bytes changed");
        check(RecordedRequestBody.from(new byte[] {'A', 0, 'B'}).mode == 5,
                "NUL must use the Base64 editor");

        byte[] maximum = new byte[RecordedRequestBody.MAX_COPY_BYTES];
        maximum[maximum.length - 1] = (byte) 255;
        encoded = RecordedRequestBody.from(maximum);
        check(encoded.value.length() > 64 * 1024 && encoded.value.length() <= 128 * 1024,
                "64 KiB binary request must fit the Base64 editor");
        check(Arrays.equals(maximum, Base64.getDecoder().decode(encoded.value)),
                "64 KiB binary request bytes changed");
        try {
            RecordedRequestBody.from(new byte[maximum.length + 1]);
            throw new AssertionError("oversize captured request accepted");
        } catch (IllegalArgumentException expected) { }
        System.out.println("RecordedRequestBodyTest passed");
    }

    private static void check(boolean condition, String description) {
        if (!condition) throw new AssertionError(description);
    }
}
