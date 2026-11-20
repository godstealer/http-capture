"""Compile actual pairing lifecycle methods with an isolated, socket-free Java mock.

This is a host method-level test, not Android TLS/VPN end-to-end evidence.
The production connect method and Android model are deliberately not replaced in
the app: only this temporary compilation uses a minimal model and queued mock IO.
"""
from pathlib import Path
import re
import subprocess
import tempfile


def java_mask(source):
    """Keep code offsets/braces while hiding comments and quoted literals."""
    pattern = r'//[^\n]*|/\*[\s\S]*?\*/|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\''
    return re.sub(pattern, lambda match: "".join(
        "\n" if char == "\n" else " " for char in match[0]), source)


def source_block(source, start, label):
    masked = java_mask(source)
    opening = masked.index("{", start)
    depth = 1
    cursor = opening + 1
    while depth:
        if cursor >= len(masked):
            raise AssertionError(f"Unbalanced actual production block: {label}")
        depth += (masked[cursor] == "{") - (masked[cursor] == "}")
        cursor += 1
    return source[start:cursor]


def method(source, name):
    masked = java_mask(source)
    match = re.search(
        rf"(?m)^    (?:private )?static [^\n;=]*\b{re.escape(name)}\([^\n]*",
        masked,
    )
    if match is None:
        raise AssertionError(f"Missing actual production method: {name}")
    return source_block(source, match.start(), name)


def relay_branch(source, signature, condition):
    masked = java_mask(source)
    start = masked.find(condition)
    if start < 0:
        raise AssertionError("Missing actual production relay guard")
    body = source_block(source, start, condition)
    return "    static void " + signature + " throws IOException {\n" + body + "\n    }"


JAVA = r"""
package dev.httpcapture.mobile;
import java.io.*;
import java.util.*;

final class LanPairingClient {
__PRODUCTION__
__RELAY_BRANCHES__

    // Only the method-test container supplies arbitrary expiry and mock sockets.
    static final class Code {}
    static final class Session {
        final Code code = new Code();
        final byte[] token = new byte[32];
        final long expiresAt;
        Session(long expiresAt) { this.expiresAt = expiresAt; Arrays.fill(token, (byte) 0x5a); }
    }
    static final Deque<SSLSocket> sockets = new ArrayDeque<>();
    static IOException connectFailure;
    static int connections;
    static boolean lastVpnMustBeActive;
    static SSLSocket connect(Code code, boolean vpnMustBeActive) throws IOException {
        connections++;
        lastVpnMustBeActive = vpnMustBeActive;
        if (connectFailure != null) {
            IOException error = connectFailure;
            connectFailure = null;
            throw error;
        }
        SSLSocket socket = sockets.poll();
        if (socket == null) throw new AssertionError("Unexpected mock connection");
        return socket;
    }
    static final class SSLSocket implements AutoCloseable {
        final ByteArrayOutputStream output = new ByteArrayOutputStream();
        final int acknowledgement;
        final IOException readFailure;
        final Runnable beforeRead;
        boolean closed;
        SSLSocket(int acknowledgement, IOException readFailure, Runnable beforeRead) {
            this.acknowledgement = acknowledgement;
            this.readFailure = readFailure;
            this.beforeRead = beforeRead;
        }
        OutputStream getOutputStream() { return output; }
        InputStream getInputStream() {
            return new InputStream() {
                @Override public int read() throws IOException {
                    if (beforeRead != null) beforeRead.run();
                    if (readFailure != null) throw readFailure;
                    return acknowledgement;
                }
            };
        }
        @Override public void close() { closed = true; }
    }
}
public final class LanPairingSessionTest {
    private static int checks;
    private interface Attempt { void run() throws Exception; }
    private static void check(boolean condition, String message) {
        checks++;
        if (!condition) throw new AssertionError(message);
    }
    private static void reject(Attempt attempt, Class<? extends Exception> kind) throws Exception {
        try {
            attempt.run();
            throw new AssertionError("Expected " + kind.getSimpleName());
        } catch (Exception error) {
            check(kind.isInstance(error), "Wrong failure kind: " + error.getClass().getSimpleName());
        }
    }
    private static LanPairingClient.Session valid() {
        return new LanPairingClient.Session(System.currentTimeMillis() + 60_000);
    }
    private static LanPairingClient.Session expired() {
        return new LanPairingClient.Session(System.currentTimeMillis() - 60_000);
    }
    private static LanPairingClient.SSLSocket answer(int acknowledgement) {
        return answer(acknowledgement, null, null);
    }
    private static LanPairingClient.SSLSocket answer(
            int acknowledgement, IOException failure, Runnable beforeRead) {
        LanPairingClient.SSLSocket socket =
            new LanPairingClient.SSLSocket(acknowledgement, failure, beforeRead);
        LanPairingClient.sockets.add(socket);
        return socket;
    }
    private static void checkFrame(LanPairingClient.SSLSocket socket, int operation) {
        byte[] bytes = socket.output.toByteArray();
        check(bytes.length >= 33 && bytes[0] == operation, "Wrong session operation");
        for (int index = 1; index <= 32; index++)
            check(bytes[index] == (byte) 0x5a, "Session token changed");
        check(socket.closed, "Session socket was not closed");
    }
    public static void main(String[] args) throws Exception {
        LanPairingClient.Session old = valid();
        LanPairingClient.Session newer = valid();
        LanPairingClient.setActiveSession(old);
        check(LanPairingClient.activeSession() == old, "Setter/getter changed session identity");
        LanPairingClient.clearActiveSession(newer);
        check(LanPairingClient.activeSession() == old, "Unrelated object cleared active pairing");
        LanPairingClient.clearActiveSession(old);
        check(LanPairingClient.activeSession() == null, "Expected pairing was not cleared");
        LanPairingClient.setActiveSession(newer);
        LanPairingClient.clearActiveSession(old);
        check(LanPairingClient.activeSession() == newer, "Old session cleared a new pairing");

        LanPairingClient.Session current = valid();
        LanPairingClient.setActiveSession(current);
        LanPairingClient.SSLSocket alive = answer(0);
        LanPairingClient.ping(current);
        check(LanPairingClient.activeSession() == current, "Valid heartbeat cleared pairing");
        check(LanPairingClient.lastVpnMustBeActive, "Heartbeat lost VPN protection requirement");
        checkFrame(alive, 3);

        LanPairingClient.SSLSocket metadata = answer(0);
        LanPairingClient.reportDeviceName(current, "Google", "Pixel");
        check(LanPairingClient.activeSession() == current, "Accepted metadata cleared pairing");
        check(!LanPairingClient.lastVpnMustBeActive, "Pre-VPN metadata requires an active VPN");
        checkFrame(metadata, 5);

        LanPairingClient.Session timedOut = expired();
        LanPairingClient.setActiveSession(timedOut);
        int before = LanPairingClient.connections;
        reject(() -> LanPairingClient.ping(timedOut), IllegalStateException.class);
        check(LanPairingClient.activeSession() == null, "Expired heartbeat retained pairing");
        check(LanPairingClient.connections == before, "Expired heartbeat opened a socket");
        LanPairingClient.setActiveSession(timedOut);
        reject(() -> LanPairingClient.reportDeviceName(timedOut, "Google", "Pixel"),
            IllegalStateException.class);
        check(LanPairingClient.activeSession() == null, "Expired metadata retained pairing");
        check(LanPairingClient.connections == before, "Expired metadata opened a socket");

        newer = valid();
        LanPairingClient.setActiveSession(newer);
        reject(() -> LanPairingClient.ping(timedOut), IllegalStateException.class);
        check(LanPairingClient.activeSession() == newer, "Expired old heartbeat cleared new pairing");
        reject(() -> LanPairingClient.reportDeviceName(timedOut, "Google", "Pixel"),
            IllegalStateException.class);
        check(LanPairingClient.activeSession() == newer, "Expired old metadata cleared new pairing");

        current = valid();
        LanPairingClient.setActiveSession(current);
        LanPairingClient.Session rejected = current;
        LanPairingClient.SSLSocket revoked = answer(1);
        reject(() -> LanPairingClient.ping(rejected), IllegalStateException.class);
        check(LanPairingClient.activeSession() == null, "PC session rejection retained pairing");
        checkFrame(revoked, 3);

        old = valid();
        newer = valid();
        LanPairingClient.Session lateHeartbeat = old;
        LanPairingClient.Session replacement = newer;
        LanPairingClient.setActiveSession(old);
        LanPairingClient.SSLSocket delayedRejection = answer(1, null,
            () -> LanPairingClient.setActiveSession(replacement));
        reject(() -> LanPairingClient.ping(lateHeartbeat), IllegalStateException.class);
        check(LanPairingClient.activeSession() == replacement,
            "Late rejected heartbeat cleared pairing installed while waiting for acknowledgement");
        checkFrame(delayedRejection, 3);

        current = valid();
        LanPairingClient.Session retryable = current;
        LanPairingClient.setActiveSession(current);
        LanPairingClient.connectFailure = new IOException("isolated connect failure");
        reject(() -> LanPairingClient.ping(retryable), IOException.class);
        check(LanPairingClient.activeSession() == current, "Connect IO failure destroyed retryable pairing");
        LanPairingClient.SSLSocket readFailed = answer(-1, new IOException("isolated read failure"), null);
        reject(() -> LanPairingClient.ping(retryable), IOException.class);
        check(LanPairingClient.activeSession() == current, "Read IO failure destroyed retryable pairing");
        checkFrame(readFailed, 3);
        LanPairingClient.SSLSocket earlyEof = answer(-1);
        reject(() -> LanPairingClient.ping(retryable), EOFException.class);
        check(LanPairingClient.activeSession() == current, "EOF was treated as a real PC rejection");
        checkFrame(earlyEof, 3);
        LanPairingClient.connectFailure = new IOException("isolated metadata connection failure");
        reject(() -> LanPairingClient.reportDeviceName(retryable, "Google", "Pixel"), IOException.class);
        check(LanPairingClient.activeSession() == current, "Metadata connect IO failure cleared pairing");

        LanPairingClient.SSLSocket oldMetadata = answer(1);
        LanPairingClient.SSLSocket fallbackAlive = answer(0);
        LanPairingClient.reportDeviceName(current, "Google", "Pixel");
        check(LanPairingClient.activeSession() == current, "Unsupported metadata cleared valid pairing");
        checkFrame(oldMetadata, 5);
        checkFrame(fallbackAlive, 3);

        LanPairingClient.SSLSocket failedMetadata = answer(-1, new IOException("old PC closed op5"), null);
        LanPairingClient.SSLSocket fallbackAfterIo = answer(0);
        LanPairingClient.reportDeviceName(current, "Google", "Pixel");
        check(LanPairingClient.activeSession() == current, "Optional metadata IO failure cleared valid pairing");
        checkFrame(failedMetadata, 5);
        checkFrame(fallbackAfterIo, 3);

        LanPairingClient.SSLSocket unsupported = answer(1);
        LanPairingClient.SSLSocket rejectedFallback = answer(1);
        reject(() -> LanPairingClient.reportDeviceName(retryable, "Google", "Pixel"),
            IllegalStateException.class);
        check(LanPairingClient.activeSession() == null, "Rejected fallback heartbeat retained pairing");
        checkFrame(unsupported, 5);
        checkFrame(rejectedFallback, 3);

        LanPairingClient.Session relayCurrent = valid();
        LanPairingClient.Session relayExpired = expired();
        LanPairingClient.Session relayNewer = valid();
        LanPairingClient.setActiveSession(relayCurrent);
        LanPairingClient.relayConstructorGuard(relayCurrent);
        LanPairingClient.relayRequestGuard(relayCurrent);
        for (int acknowledgement : new int[] {0, 2, 255}) {
            LanPairingClient.relayAcknowledgementGuard(relayCurrent, acknowledgement);
            check(LanPairingClient.activeSession() == relayCurrent,
                "Non-authentication relay response cleared pairing");
        }
        reject(() -> LanPairingClient.relayConstructorGuard(null), IllegalStateException.class);
        check(LanPairingClient.activeSession() == relayCurrent,
            "Missing relay session cleared an unrelated pairing");
        reject(() -> LanPairingClient.relayAcknowledgementGuard(relayCurrent, 1),
            IllegalStateException.class);
        check(LanPairingClient.activeSession() == null, "Relay authentication rejection retained pairing");
        LanPairingClient.setActiveSession(relayNewer);
        reject(() -> LanPairingClient.relayAcknowledgementGuard(relayCurrent, 1),
            IllegalStateException.class);
        check(LanPairingClient.activeSession() == relayNewer,
            "Old relay authentication rejection cleared new pairing");

        LanPairingClient.setActiveSession(relayExpired);
        reject(() -> LanPairingClient.relayConstructorGuard(relayExpired), IllegalStateException.class);
        check(LanPairingClient.activeSession() == null, "Expired relay startup retained pairing");
        LanPairingClient.setActiveSession(relayExpired);
        reject(() -> LanPairingClient.relayRequestGuard(relayExpired), IOException.class);
        check(LanPairingClient.activeSession() == null, "Expired relay request retained pairing");
        LanPairingClient.setActiveSession(relayNewer);
        reject(() -> LanPairingClient.relayConstructorGuard(relayExpired), IllegalStateException.class);
        check(LanPairingClient.activeSession() == relayNewer, "Expired old relay startup cleared new pairing");
        reject(() -> LanPairingClient.relayRequestGuard(relayExpired), IOException.class);
        check(LanPairingClient.activeSession() == relayNewer, "Expired old relay request cleared new pairing");
        check(LanPairingClient.sockets.isEmpty(), "Unexpected unconsumed socket attempts");
        LanPairingClient.setActiveSession(null);
        System.out.println("LanPairingSessionTest passed (" + checks + " checks; production methods, mock IO)");
    }
}
"""


def main():
    repository = Path(__file__).resolve().parents[4]
    java_source = repository / "apps/mobile/android/app/src/main/java/dev/httpcapture/mobile"
    production = (java_source / "LanPairingClient.java").read_text(encoding="utf-8")
    field = re.search(r"(?m)^    private static volatile Session activeSession;", production)
    if field is None:
        raise AssertionError("Missing actual production session field")
    members = [field[0]] + [
        method(production, name) for name in (
            "setActiveSession", "activeSession", "clearActiveSession",
            "reportDeviceName", "ping", "verifySession",
        )
    ]
    relay = (java_source / "ProtectedRelay.java").read_text(encoding="utf-8")
    relay_guards = [
        relay_branch(relay, signature, condition) for signature, condition in (
            ("relayConstructorGuard(Session session)",
             "if (session == null || session.expiresAt <= System.currentTimeMillis())"),
            ("relayRequestGuard(Session session)",
             "if (session.expiresAt <= System.currentTimeMillis())"),
            ("relayAcknowledgementGuard(Session session, int result)", "if (result == 1)"),
        )
    ]
    with tempfile.TemporaryDirectory(prefix="http-capture-pairing-session-") as temporary:
        directory = Path(temporary)
        generated = directory / "LanPairingSessionTest.java"
        generated.write_text(JAVA.replace("__PRODUCTION__", "\n".join(members))
            .replace("__RELAY_BRANCHES__", "\n".join(relay_guards)), encoding="utf-8")
        subprocess.run([
            "javac", "-encoding", "UTF-8", "-d", str(directory),
            str(generated), str(java_source / "DeviceNameCodec.java"),
        ], check=True)
        subprocess.run([
            "java", "-cp", str(directory), "dev.httpcapture.mobile.LanPairingSessionTest",
        ], check=True)


if __name__ == "__main__":
    main()
