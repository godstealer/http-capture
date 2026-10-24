package dev.httpcapture.mobile;

import android.app.Activity;
import android.app.Instrumentation;
import android.os.Bundle;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.atomic.AtomicReference;

/** Exercises the packaged JNI library, loopback proxy and persistent capture on an emulator. */
public final class CaptureSmokeInstrumentation extends Instrumentation {
    @Override public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        start();
    }

    @Override public void onStart() {
        Bundle result = new Bundle();
        try {
            runSmoke();
            result.putString("smokeStatus", "ok");
            finish(Activity.RESULT_OK, result);
        } catch (Throwable error) {
            result.putString("smokeStatus", "failed: " + error);
            finish(Activity.RESULT_CANCELED, result);
        }
    }

    private void runSmoke() throws Exception {
        try (ServerSocket origin = new ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"));
             ServerSocket reservation = new ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"))) {
            int proxyPort = reservation.getLocalPort();
            reservation.close();
            expectOk(NativeCapture.start(getTargetContext().getFilesDir().getAbsolutePath(), proxyPort));
            try {
                String certificate = new JSONObject(NativeCapture.certificate()).getString("value");
                if (!certificate.contains("BEGIN CERTIFICATE") || certificate.contains("PRIVATE KEY")) {
                    throw new AssertionError("CA export must contain only a public certificate");
                }

                AtomicReference<Throwable> originError = new AtomicReference<>();
                Thread server = new Thread(() -> {
                    try (Socket peer = origin.accept()) {
                        peer.setSoTimeout(10_000);
                        InputStream input = peer.getInputStream();
                        ByteArrayOutputStream head = new ByteArrayOutputStream();
                        int byteRead;
                        while ((byteRead = input.read()) != -1) {
                            head.write(byteRead);
                            byte[] bytes = head.toByteArray();
                            int size = bytes.length;
                            if (size >= 4 && bytes[size - 4] == '\r' && bytes[size - 3] == '\n'
                                    && bytes[size - 2] == '\r' && bytes[size - 1] == '\n') break;
                            if (size > 16 * 1024) throw new AssertionError("oversized request head");
                        }
                        String request = head.toString(StandardCharsets.US_ASCII.name());
                        if (!request.startsWith("GET /mobile-smoke HTTP/1.1\r\n")) {
                            throw new AssertionError("unexpected origin request: " + request);
                        }
                        peer.getOutputStream().write(("HTTP/1.1 200 OK\r\nContent-Length: 8\r\n"
                                + "Connection: close\r\n\r\nsmoke-ok").getBytes(StandardCharsets.US_ASCII));
                    } catch (Throwable error) {
                        originError.set(error);
                    }
                }, "capture-smoke-origin");
                server.start();

                try (Socket client = new Socket("127.0.0.1", proxyPort)) {
                    client.setSoTimeout(10_000);
                    OutputStream output = client.getOutputStream();
                    String target = "127.0.0.1:" + origin.getLocalPort();
                    output.write(("GET http://" + target + "/mobile-smoke HTTP/1.1\r\n"
                            + "Host: " + target + "\r\nConnection: close\r\n\r\n")
                            .getBytes(StandardCharsets.US_ASCII));
                    ByteArrayOutputStream response = new ByteArrayOutputStream();
                    byte[] buffer = new byte[4096];
                    int count;
                    InputStream input = client.getInputStream();
                    while ((count = input.read(buffer)) != -1) response.write(buffer, 0, count);
                    String text = response.toString(StandardCharsets.US_ASCII.name());
                    if (!text.startsWith("HTTP/1.1 200") || !text.endsWith("smoke-ok")) {
                        throw new AssertionError("unexpected proxy response: " + text);
                    }
                }
                server.join(10_000);
                if (server.isAlive()) throw new AssertionError("origin did not finish");
                if (originError.get() != null) throw new AssertionError("origin failed", originError.get());

                long deadline = System.currentTimeMillis() + 10_000;
                boolean recorded = false;
                while (System.currentTimeMillis() < deadline) {
                    JSONObject page = expectOk(NativeCapture.recent("", "mobile-smoke"));
                    JSONArray requests = page.getJSONArray("requests");
                    if (requests.length() > 0) {
                        recorded = true;
                        break;
                    }
                    Thread.sleep(100);
                }
                if (!recorded) throw new AssertionError("proxy response was not recorded");
            } finally {
                expectOk(NativeCapture.stop());
            }
        }
    }

    private static JSONObject expectOk(String reply) throws Exception {
        JSONObject parsed = new JSONObject(reply);
        if (!parsed.getBoolean("ok")) throw new AssertionError(parsed.optString("error", reply));
        return parsed.optJSONObject("value");
    }
}
