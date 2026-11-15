package dev.httpcapture.mobile;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;

/** Run with javac/java on a host JDK; no device, native capture library or Android APIs needed. */
public final class DeviceNameCodecTest {
    private static int checks;

    public static void main(String[] args) throws Exception {
        checkName(null, null, "Android");
        checkName("  UnKnOwN ", " unknown ", "Android");
        checkName("", "Pixel 10", "Pixel 10");
        checkName("Google", null, "Google");
        checkName("Google", "google", "Google");
        checkName("Google", "gOoGlE Pixel 10", "gOoGlE Pixel 10");
        checkName("LG", "LGC100", "LG LGC100");
        checkName("  Google\u00a0 ", "\u3000Pixel   10  ", "Google Pixel 10");

        StringBuilder forbidden = new StringBuilder();
        for (int codePoint = 0; codePoint <= 31; codePoint++) forbidden.appendCodePoint(codePoint);
        for (int codePoint = 127; codePoint <= 159; codePoint++) forbidden.appendCodePoint(codePoint);
        for (int codePoint : new int[] {0x061c, 0x200e, 0x200f, 0x2028, 0x2029,
                0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066, 0x2067, 0x2068, 0x2069})
            forbidden.appendCodePoint(codePoint);
        checkName(null, "left" + forbidden + "right", "leftright");
        checkName(forbidden.toString(), forbidden + "unknown", "Android");
        checkName("Go\u202eogle", "Google Pixel", "Google Pixel");
        checkName(null, "A\ud800B\udc00C\ud83d\udcf1", "ABC\ud83d\udcf1");

        checkName(null, "A".repeat(128), "A".repeat(128));
        checkName(null, "A".repeat(129), "A".repeat(128));
        checkName(null, "\u4e2d".repeat(42) + "AB", "\u4e2d".repeat(42) + "AB");
        checkName(null, "\u4e2d".repeat(43), "\u4e2d".repeat(42));
        checkName(null, "A".repeat(127) + "\ud83d\udcf1", "A".repeat(127));
        checkName(null, "\ud83d\udcf1".repeat(33), "\ud83d\udcf1".repeat(32));
        checkName(null, "A".repeat(127) + " B", "A".repeat(127));

        checkFrame("Google", "Pixel", "Google Pixel");
        checkFrame("A", "\u4e2d\ud83d\udcf1", "A \u4e2d\ud83d\udcf1");
        byte[] maximum = checkFrame(null, "X".repeat(129), "X".repeat(128));
        check((maximum[33] & 0xff) == 0 && (maximum[34] & 0xff) == 128,
            "frame length must be an unsigned network-order byte count");
        checkFrame(null, "\u4e2d".repeat(43), "\u4e2d".repeat(42));
        checkFrame(null, "\ud83d\udcf1".repeat(33), "\ud83d\udcf1".repeat(32));
        checkFrame(null, "left" + forbidden + "right\ud800", "leftright");
        checkFrame(null, null, "Android");

        rejectToken(null);
        rejectToken(new byte[31]);
        rejectToken(new byte[33]);
        try {
            DeviceNameCodec.writeReport(new OutputStream() {
                @Override public void write(int value) throws IOException { throw new IOException("write failed"); }
            }, token(), null, "Pixel");
            throw new AssertionError("write failures were swallowed");
        } catch (IOException expected) {
            check(expected.getMessage().equals("write failed"), "write failure changed");
        }
        try {
            DeviceNameCodec.writeReport(new ByteArrayOutputStream() {
                @Override public void flush() throws IOException { throw new IOException("flush failed"); }
            }, token(), null, "Pixel");
            throw new AssertionError("flush failures were swallowed");
        } catch (IOException expected) {
            check(expected.getMessage().equals("flush failed"), "flush failure changed");
        }
        System.out.println("DeviceNameCodecTest passed (" + checks + " checks)");
    }

    private static void checkName(String manufacturer, String model, String expected) {
        String actual = DeviceNameCodec.deviceName(manufacturer, model);
        check(expected.equals(actual), "unexpected device name");
        check(!actual.isEmpty() && actual.getBytes(StandardCharsets.UTF_8).length <= 128,
            "device name must be nonempty and fit the UTF-8 byte limit");
    }

    private static byte[] checkFrame(String manufacturer, String model, String expected) throws Exception {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        byte[] sessionToken = token();
        DeviceNameCodec.writeReport(output, sessionToken, manufacturer, model);
        byte[] bytes = output.toByteArray();
        DataInputStream frame = new DataInputStream(new ByteArrayInputStream(bytes));
        check(frame.readUnsignedByte() == 5, "wrong operation");
        byte[] actualToken = new byte[32];
        frame.readFully(actualToken);
        check(Arrays.equals(sessionToken, actualToken), "session token changed");
        int length = frame.readUnsignedShort();
        check(length == expected.getBytes(StandardCharsets.UTF_8).length,
            "wire length must count UTF-8 bytes");
        byte[] name = new byte[length];
        frame.readFully(name);
        String actual = StandardCharsets.UTF_8.newDecoder()
            .onMalformedInput(CodingErrorAction.REPORT).onUnmappableCharacter(CodingErrorAction.REPORT)
            .decode(ByteBuffer.wrap(name)).toString();
        check(expected.equals(actual), "wire name differs from the normalized description");
        check(frame.available() == 0 && bytes.length == 35 + length, "unexpected frame suffix");
        return bytes;
    }

    private static byte[] token() {
        byte[] token = new byte[32];
        for (int index = 0; index < token.length; index++) token[index] = (byte) (0xa0 + index);
        return token;
    }

    private static void rejectToken(byte[] token) throws Exception {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        try {
            DeviceNameCodec.writeReport(output, token, "Google", "Pixel");
            throw new AssertionError("invalid session token accepted");
        } catch (IllegalArgumentException expected) {
            check(output.size() == 0, "invalid token emitted a partial frame");
        }
    }

    private static void check(boolean condition, String message) {
        checks++;
        if (!condition) throw new AssertionError(message);
    }
}
