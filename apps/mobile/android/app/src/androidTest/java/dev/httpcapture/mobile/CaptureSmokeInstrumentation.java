package dev.httpcapture.mobile;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.ComponentName;
import android.content.Intent;
import android.net.VpnService;
import android.os.Bundle;
import android.util.Base64;
import android.view.accessibility.AccessibilityNodeInfo;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.atomic.AtomicReference;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLParameters;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManagerFactory;
import java.security.KeyStore;
import java.security.cert.CertificateFactory;

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
            runVpnLifecycleSmoke();
            result.putString("smokeStatus", "ok");
            finish(Activity.RESULT_OK, result);
        } catch (Throwable error) {
            result.putString("smokeStatus", "failed: " + error);
            finish(Activity.RESULT_CANCELED, result);
        }
    }

    private void runVpnLifecycleSmoke() throws Exception {
        Intent consent = VpnService.prepare(getTargetContext());
        Activity consentHost = null;
        if (consent != null) {
            Activity activity = startActivitySync(new Intent(getTargetContext(), MainActivity.class)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            consentHost = activity;
            activity.runOnUiThread(() -> activity.startActivityForResult(consent, 49123));
            long deadline = System.currentTimeMillis() + 30_000;
            boolean approved = false;
            String visibleWindow = "no accessibility window";
            while (System.currentTimeMillis() < deadline) {
                AccessibilityNodeInfo root = getUiAutomation().getRootInActiveWindow();
                if (root != null) visibleWindow = describeWindow(root);
                AccessibilityNodeInfo button = findConsentButton(root);
                if (button != null && button.performAction(AccessibilityNodeInfo.ACTION_CLICK)) {
                    approved = true;
                    break;
                }
                Thread.sleep(200);
            }
            if (!approved) throw new AssertionError("VPN consent button was not available: " + visibleWindow);
        }
        long permissionDeadline = System.currentTimeMillis() + 10_000;
        while (System.currentTimeMillis() < permissionDeadline
                && VpnService.prepare(getTargetContext()) != null) Thread.sleep(100);
        if (VpnService.prepare(getTargetContext()) != null) {
            throw new AssertionError("VPN consent was not granted");
        }
        Activity host = consentHost;
        if (host != null) host.runOnUiThread(host::finish);

        try (ServerSocket reservation = new ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"))) {
            int port = reservation.getLocalPort();
            reservation.close();
            Intent start = new Intent(getTargetContext(), CaptureVpnService.class);
            start.setAction(CaptureVpnService.ACTION_START);
            start.putExtra(CaptureVpnService.EXTRA_PORT, port);
            getTargetContext().startForegroundService(start);
            long deadline = System.currentTimeMillis() + 30_000;
            while (System.currentTimeMillis() < deadline && !CaptureVpnService.isActive()) {
                if (!CaptureVpnService.isManaged() && CaptureVpnService.isStopping()) {
                    throw new AssertionError(CaptureVpnService.message());
                }
                Thread.sleep(200);
            }
            if (!CaptureVpnService.isActive()) {
                throw new AssertionError("VPN did not establish: " + CaptureVpnService.message());
            }
            expectOk(NativeCapture.tunnelStatus());
            Set<String> previousIDs = new HashSet<>();
            JSONArray previous = expectOk(NativeCapture.recent("", "vpn-smoke")).getJSONArray("requests");
            for (int i = 0; i < previous.length(); i++) {
                previousIDs.add(previous.getJSONObject(i).optString("id"));
            }
            Intent probe = new Intent("dev.httpcapture.probe.RUN");
            probe.setComponent(new ComponentName("dev.httpcapture.probe",
                    "dev.httpcapture.probe.ProbeReceiver"));
            getTargetContext().sendBroadcast(probe);
            long trafficDeadline = System.currentTimeMillis() + 20_000;
            boolean captured = false;
            String lastState = "请求尚未出现";
            while (System.currentTimeMillis() < trafficDeadline) {
                JSONObject page = expectOk(NativeCapture.recent("", "vpn-smoke"));
                JSONArray requests = page.getJSONArray("requests");
                for (int i = 0; i < requests.length(); i++) {
                    JSONObject summary = requests.getJSONObject(i);
                    if (previousIDs.contains(summary.optString("id"))) continue;
                    lastState = summary.optString("state") + " · HTTP " + summary.optString("status")
                        + " · " + summary.optString("error");
                    // A pending capture record can appear before the proxy has
                    // sent the request to the origin. Do not stop the VPN yet.
                    if (!summary.isNull("status")) {
                        captured = true;
                        break;
                    }
                }
                if (captured) break;
                Thread.sleep(200);
            }
            if (!captured) throw new AssertionError("VPN raw TCP request did not finish: " + lastState);
            runVpnTlsSmoke();
        } finally {
            Intent stop = new Intent(getTargetContext(), CaptureVpnService.class);
            stop.setAction(CaptureVpnService.ACTION_STOP);
            getTargetContext().startService(stop);
            long deadline = System.currentTimeMillis() + 30_000;
            while (System.currentTimeMillis() < deadline && CaptureVpnService.isManaged()) {
                Thread.sleep(200);
            }
            if (CaptureVpnService.isManaged()) {
                throw new AssertionError("VPN did not stop: " + CaptureVpnService.message());
            }
        }
    }

    private void runVpnTlsSmoke() throws Exception {
        JSONObject previousBlocking = expectOk(NativeCapture.blocking());
        try {
            JSONObject rule = new JSONObject().put("id", "vpn-tls-smoke")
                    .put("enabled", true).put("matchType", "urlContains")
                    .put("pattern", "/vpn-tls-smoke").put("method", "GET");
            JSONObject config = new JSONObject().put("enabled", true)
                    .put("scope", "capture").put("rules", new JSONArray().put(rule));
            expectOk(NativeCapture.configureBlocking(config.toString()));
            Set<String> previousIDs = new HashSet<>();
            JSONArray previous = expectOk(NativeCapture.recent("", "vpn-tls-smoke"))
                    .getJSONArray("requests");
            for (int i = 0; i < previous.length(); i++) {
                previousIDs.add(previous.getJSONObject(i).optString("id"));
            }
            Intent probe = new Intent("dev.httpcapture.probe.RUN_TLS");
            probe.setComponent(new ComponentName("dev.httpcapture.probe",
                    "dev.httpcapture.probe.ProbeReceiver"));
            probe.putExtra("caPem", new JSONObject(NativeCapture.certificate()).getString("value"));
            getTargetContext().sendBroadcast(probe);
            long deadline = System.currentTimeMillis() + 30_000;
            String lastState = "TLS 请求尚未出现";
            while (System.currentTimeMillis() < deadline) {
                JSONArray page = expectOk(NativeCapture.recent("", "vpn-tls-smoke"))
                        .getJSONArray("requests");
                for (int i = 0; i < page.length(); i++) {
                    JSONObject summary = page.getJSONObject(i);
                    if (previousIDs.contains(summary.optString("id"))) continue;
                    lastState = summary.optString("state") + " · HTTP " + summary.optString("status")
                            + " · " + summary.optString("error");
                    if ("blocked".equals(summary.optString("state"))
                            && summary.optInt("status") == 403) return;
                }
                Thread.sleep(200);
            }
            throw new AssertionError("VPN TLS interception failed: " + lastState);
        } finally {
            expectOk(NativeCapture.configureBlocking(previousBlocking.toString()));
        }
    }

    private static AccessibilityNodeInfo findConsentButton(AccessibilityNodeInfo node) {
        if (node == null) return null;
        CharSequence id = node.getViewIdResourceName();
        String resource = id == null ? "" : id.toString();
        if ("android:id/button1".equals(resource) || resource.endsWith(":id/button_start_vpn")) return node;
        CharSequence label = node.getText();
        String text = label == null ? "" : label.toString().trim();
        if ("android.widget.Button".contentEquals(node.getClassName() == null ? "" : node.getClassName())
                && ("OK".equalsIgnoreCase(text) || "Allow".equalsIgnoreCase(text)
                || "Continue".equalsIgnoreCase(text) || "确定".equals(text) || "允许".equals(text))) return node;
        for (int index = 0; index < node.getChildCount(); index++) {
            AccessibilityNodeInfo found = findConsentButton(node.getChild(index));
            if (found != null) return found;
        }
        return null;
    }

    private static String describeWindow(AccessibilityNodeInfo root) {
        StringBuilder details = new StringBuilder();
        describeNode(root, details);
        return details.length() > 800 ? details.substring(0, 800) : details.toString();
    }

    private static void describeNode(AccessibilityNodeInfo node, StringBuilder details) {
        if (node == null || details.length() > 800) return;
        details.append('[').append(node.getClassName()).append(' ')
                .append(node.getViewIdResourceName()).append(' ')
                .append(node.getText()).append(']');
        for (int index = 0; index < node.getChildCount(); index++) {
            describeNode(node.getChild(index), details);
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
                runCertificateManagementSmoke(certificate);

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
                runTlsSmoke(proxyPort, certificate);
            } finally {
                expectOk(NativeCapture.stop());
            }
        }
    }

    private void runCertificateManagementSmoke(String certificate) throws Exception {
        JSONObject before = expectOk(NativeCapture.certificates());
        JSONObject request = new JSONObject().put("name", "Android smoke CA")
                .put("kind", "trust").put("host", "localhost").put("port", 443)
                .put("certificateBase64", Base64.encodeToString(
                        certificate.getBytes(StandardCharsets.US_ASCII), Base64.NO_WRAP))
                .put("expectedVersion", before.getString("version"));
        JSONObject imported = expectOk(NativeCapture.importCertificate(request.toString()));
        String id = null;
        for (int index = 0; index < imported.getJSONArray("entries").length(); index++) {
            JSONObject entry = imported.getJSONArray("entries").getJSONObject(index);
            if ("Android smoke CA".equals(entry.optString("name"))) id = entry.getString("id");
        }
        if (id == null) throw new AssertionError("imported trust CA is missing");
        try {
            JSONObject change = new JSONObject().put("action", "enable").put("id", id)
                    .put("enabled", true).put("expectedVersion", imported.getString("version"));
            JSONObject enabled = expectOk(NativeCapture.changeCertificate(change.toString()));
            boolean foundEnabled = false;
            for (int index = 0; index < enabled.getJSONArray("entries").length(); index++) {
                JSONObject entry = enabled.getJSONArray("entries").getJSONObject(index);
                if (id.equals(entry.optString("id"))) foundEnabled = entry.getBoolean("enabled");
            }
            if (!foundEnabled) throw new AssertionError("trust CA was not enabled");
        } finally {
            JSONObject current = expectOk(NativeCapture.certificates());
            JSONObject delete = new JSONObject().put("action", "delete").put("id", id)
                    .put("expectedVersion", current.getString("version"));
            expectOk(NativeCapture.changeCertificate(delete.toString()));
        }
    }

    private void runTlsSmoke(int proxyPort, String certificate) throws Exception {
        JSONObject previousBlocking = expectOk(NativeCapture.blocking());
        try (ServerSocket reservation = new ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"))) {
            int unusedOriginPort = reservation.getLocalPort();
            reservation.close();
            String target = "127.0.0.1:" + unusedOriginPort;
            String path = "/mobile-tls-smoke";
            JSONObject rule = new JSONObject().put("id", "mobile-tls-smoke")
                    .put("enabled", true).put("matchType", "urlContains")
                    .put("pattern", path).put("method", "GET");
            JSONObject blocking = new JSONObject().put("enabled", true)
                    .put("scope", "capture").put("rules", new JSONArray().put(rule));
            expectOk(NativeCapture.configureBlocking(blocking.toString()));
            try {
                CertificateFactory parser = CertificateFactory.getInstance("X.509");
                KeyStore roots = KeyStore.getInstance(KeyStore.getDefaultType());
                roots.load(null);
                roots.setCertificateEntry("capture-test-ca", parser.generateCertificate(
                        new java.io.ByteArrayInputStream(certificate.getBytes(StandardCharsets.US_ASCII))));
                TrustManagerFactory trust = TrustManagerFactory.getInstance(
                        TrustManagerFactory.getDefaultAlgorithm());
                trust.init(roots);
                SSLContext tls = SSLContext.getInstance("TLS");
                tls.init(null, trust.getTrustManagers(), null);

                try (Socket proxy = new Socket("127.0.0.1", proxyPort)) {
                    proxy.setSoTimeout(10_000);
                    proxy.getOutputStream().write(("CONNECT " + target + " HTTP/1.1\r\n"
                            + "Host: " + target + "\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
                    ByteArrayOutputStream connectHead = new ByteArrayOutputStream();
                    InputStream input = proxy.getInputStream();
                    int next;
                    while ((next = input.read()) != -1) {
                        connectHead.write(next);
                        String head = connectHead.toString(StandardCharsets.US_ASCII.name());
                        if (head.endsWith("\r\n\r\n")) break;
                        if (connectHead.size() > 4096) throw new AssertionError("oversized CONNECT response");
                    }
                    if (!connectHead.toString(StandardCharsets.US_ASCII.name())
                            .startsWith("HTTP/1.1 200")) throw new AssertionError("CONNECT failed");
                    try (SSLSocket client = (SSLSocket) tls.getSocketFactory()
                            .createSocket(proxy, "127.0.0.1", unusedOriginPort, false)) {
                        client.setSoTimeout(10_000);
                        SSLParameters params = client.getSSLParameters();
                        params.setEndpointIdentificationAlgorithm("HTTPS");
                        client.setSSLParameters(params);
                        client.startHandshake();
                        client.getOutputStream().write(("GET " + path + " HTTP/1.1\r\n"
                                + "Host: " + target + "\r\nConnection: close\r\n\r\n")
                                .getBytes(StandardCharsets.US_ASCII));
                        ByteArrayOutputStream response = new ByteArrayOutputStream();
                        byte[] buffer = new byte[4096];
                        int count;
                        while ((count = client.getInputStream().read(buffer)) != -1) {
                            response.write(buffer, 0, count);
                            if (response.size() > 64 * 1024) throw new AssertionError("oversized TLS response");
                        }
                        String text = response.toString(StandardCharsets.US_ASCII.name());
                        if (!text.startsWith("HTTP/1.1 403") || !text.contains("X-HTTP-Capture-Blocked: mobile-tls-smoke")) {
                            throw new AssertionError("decrypted TLS request was not blocked: " + text);
                        }
                    }
                }
                JSONArray captured = expectOk(NativeCapture.recent("", path)).getJSONArray("requests");
                if (captured.length() == 0 || !"blocked".equals(captured.getJSONObject(0).optString("state"))) {
                    throw new AssertionError("decrypted TLS request was not recorded");
                }
            } finally {
                expectOk(NativeCapture.configureBlocking(previousBlocking.toString()));
            }
        }
    }

    private static JSONObject expectOk(String reply) throws Exception {
        JSONObject parsed = new JSONObject(reply);
        if (!parsed.getBoolean("ok")) throw new AssertionError(parsed.optString("error", reply));
        return parsed.optJSONObject("value");
    }
}
