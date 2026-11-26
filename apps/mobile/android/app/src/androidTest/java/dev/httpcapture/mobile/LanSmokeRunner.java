package dev.httpcapture.mobile;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.BroadcastReceiver;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.net.VpnService;
import android.os.Build;
import android.os.Bundle;
import android.os.Process;
import android.os.SystemClock;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructStat;
import android.util.AtomicFile;
import android.util.Base64;
import android.view.accessibility.AccessibilityNodeInfo;

import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.nio.charset.StandardCharsets;
import java.security.cert.CertificateException;
import java.util.Arrays;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/** Opt-in emulator E2E. Credentials enter only through an app-private 0600 file. */
final class LanSmokeRunner {
    private static final String PROBE_ACTION = "dev.httpcapture.probe.RUN_LAN";
    private static final String PROBE_RESULT = "dev.httpcapture.mobile.LAN_PROBE_RESULT";
    private final Instrumentation instrumentation;
    private final Context context;
    private final Bundle arguments;
    private File configFile, phaseFile;
    private String runId, phase = "config", probeFailureType;
    private boolean ownsSession, startedVpn;
    private int proxyPort;

    LanSmokeRunner(Instrumentation instrumentation, Bundle arguments) {
        this.instrumentation = instrumentation;
        this.context = instrumentation.getTargetContext();
        this.arguments = arguments;
    }

    void run() {
        Bundle result = new Bundle();
        boolean success = false;
        try {
            configFile = privatePath(arguments.getString("lanConfigPath"));
            phaseFile = privatePath(arguments.getString("lanPhasePath"));
            require(!configFile.equals(phaseFile));
            JSONObject config = readAndDeleteConfig(configFile);
            require(config.getInt("schemaVersion") == 1);
            runId = config.getString("runId");
            require(runId.matches("[0-9a-f]{32}"));
            String pairingUri = config.getString("pairingUri");
            LanPairingClient.Code code = LanPairingClient.parse(pairingUri);
            require(code.host.equals(config.getString("host")) && code.port == config.getInt("gatewayPort"));
            int httpPort = port(config.getInt("httpPort"));
            int udpPort = port(config.getInt("udpPort"));
            int dnsWirePort = port(config.getInt("dnsWirePort"));
            checkedBody(config.getString("expectedRequestBodyBase64"));
            checkedBody(config.getString("expectedResponseBodyBase64"));
            writePhase("config-loaded");
            require(!CaptureVpnService.isActive() && !CaptureVpnService.isManaged() && !CaptureVpnService.isStopping());
            require(LanPairingClient.activeSession() == null);
            requireEngineStopped();

            phase = "pin-check";
            byte[] wrongFingerprint = code.fingerprint.clone();
            wrongFingerprint[0] ^= 1;
            LanPairingClient.Code wrong = new LanPairingClient.Code(code.host, code.port, code.token.clone(), wrongFingerprint, code.expiresAt);
            boolean pinRejected = false;
            try { LanPairingClient.redeem(wrong); }
            catch (Exception expected) {
                // A timeout or ordinary connect failure cannot pass a pin test.
                for (Throwable cause = expected; cause != null; cause = cause.getCause())
                    if (cause instanceof CertificateException) pinRejected = true;
            }
            require(pinRejected);
            writePhase("pin-rejected");

            phase = "pairing";
            LanPairingClient.Session session = LanPairingClient.redeem(code);
            LanPairingClient.reportDeviceName(session, Build.MANUFACTURER, Build.MODEL);
            LanPairingClient.setActiveSession(session);
            ownsSession = true;
            boolean replayRejected = false;
            try { LanPairingClient.redeem(code); }
            catch (IllegalStateException expected) { replayRejected = true; }
            require(replayRejected);
            writePhase("replay-rejected");

            phase = "vpn-consent";
            grantVpnConsent();
            writePhase("vpn-consent-granted");
            phase = "vpn-start";
            try (ServerSocket reservation = new ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"))) {
                proxyPort = reservation.getLocalPort();
            }
            Intent start = new Intent(context, CaptureVpnService.class).setAction(CaptureVpnService.ACTION_START);
            start.putExtra(CaptureVpnService.EXTRA_PORT, proxyPort);
            start.putExtra(CaptureVpnService.EXTRA_REMOTE_MODE, true);
            start.putExtra(CaptureVpnService.EXTRA_ALLOWED_PACKAGES, new String[] {"dev.httpcapture.probe"});
            start.putExtra(CaptureVpnService.EXTRA_SYSTEM_PROXY_HINT, false);
            startedVpn = true;
            context.startForegroundService(start);
            writePhase("vpn-start-requested");
            long readyDeadline = SystemClock.elapsedRealtime() + 30000;
            while (SystemClock.elapsedRealtime() < readyDeadline && !vpnReady()) Thread.sleep(100);
            require(vpnReady());
            writePhase("vpn-ready");

            phase = "probe";
            runProbe(config, httpPort, udpPort, dnsWirePort);
            require(vpnReady());
            writePhase("traffic-ok");

            // The host now verifies actual PC rows before it calls disconnect.
            // Do not stop the VPN ourselves or mistake an origin's EOF for revoke.
            phase = "revocation";
            long stopDeadline = SystemClock.elapsedRealtime() + 90000;
            while (SystemClock.elapsedRealtime() < stopDeadline &&
                    (CaptureVpnService.isActive() || CaptureVpnService.isManaged() || CaptureVpnService.isStopping()))
                Thread.sleep(100);
            require(!CaptureVpnService.isActive() && !CaptureVpnService.isManaged() && !CaptureVpnService.isStopping());
            requireEngineStopped();
            JSONObject tunnel = new JSONObject(NativeCapture.tunnelStatus());
            require(!tunnel.optBoolean("ok") && "Capture is not running".equals(tunnel.optString("error")));
            require(LanPairingClient.activeSession() == null);
            try (ServerSocket released = new ServerSocket()) {
                released.setReuseAddress(false);
                released.bind(new InetSocketAddress(InetAddress.getByName("127.0.0.1"), proxyPort));
            }
            // Still connect to the pinned live gateway after revocation. Its
            // production metadata fallback verifies op3 and must reject this
            // old session, independently of closed HTTP/UDP fixture origins.
            boolean sessionRejected = false;
            try { LanPairingClient.reportDeviceName(session, Build.MANUFACTURER, Build.MODEL); }
            catch (IllegalStateException expected) { sessionRejected = true; }
            require(sessionRejected);
            writePhase("revoked-ok");
            result.putString("lanSmokeStatus", "ok");
            result.putString("runId", runId);
            result.putBoolean("lanPinRejected", true);
            result.putBoolean("lanReplayRejected", true);
            success = true;
        } catch (Throwable error) {
            String exceptionType = probeFailureType == null ? error.getClass().getSimpleName() : probeFailureType;
            result.putString("lanSmokeStatus", "failed:" + phase + ":" + exceptionType);
            if (runId != null && runId.matches("[0-9a-f]{32}")) result.putString("runId", runId);
            result.putString("lanPhase", phase);
            // No Throwable.toString(), getMessage(), pairing URI or token reaches
            // instrumentation output, logs, status files or exception Bundles.
            result.putString("lanExceptionType", exceptionType);
            try { if (runId != null && runId.matches("[0-9a-f]{32}") && phaseFile != null) writePhase("failed"); } catch (Throwable ignored) { }
        } finally {
            if (startedVpn && (CaptureVpnService.isActive() || CaptureVpnService.isManaged() || CaptureVpnService.isStopping())) {
                try {
                    context.startService(new Intent(context, CaptureVpnService.class).setAction(CaptureVpnService.ACTION_STOP));
                    long deadline = SystemClock.elapsedRealtime() + 15000;
                    while (SystemClock.elapsedRealtime() < deadline &&
                            (CaptureVpnService.isActive() || CaptureVpnService.isManaged() || CaptureVpnService.isStopping())) Thread.sleep(100);
                } catch (Throwable ignored) { }
            }
            if (ownsSession) LanPairingClient.setActiveSession(null);
            if (configFile != null) configFile.delete();
            // The host removes the sanitized phase file after reading terminal status.
        }
        instrumentation.finish(success ? Activity.RESULT_OK : Activity.RESULT_CANCELED, result);
    }

    private JSONObject readAndDeleteConfig(File file) throws Exception {
        StructStat stat = Os.lstat(file.getAbsolutePath());
        require(OsConstants.S_ISREG(stat.st_mode) && (stat.st_mode & 0777) == 0600 && stat.st_uid == Process.myUid());
        byte[] bytes;
        try (FileInputStream input = new FileInputStream(file); ByteArrayOutputStream output = new ByteArrayOutputStream()) {
            byte[] buffer = new byte[4096];
            for (int count; (count = input.read(buffer)) != -1;) {
                require(output.size() + count <= 196608);
                output.write(buffer, 0, count);
            }
            bytes = output.toByteArray();
        } finally {
            require(file.delete());
        }
        try { return new JSONObject(new String(bytes, StandardCharsets.UTF_8)); }
        finally { Arrays.fill(bytes, (byte) 0); }
    }
    private File privatePath(String path) throws Exception {
        require(path != null && new File(path).isAbsolute());
        File root = context.getFilesDir().getCanonicalFile();
        File file = new File(path).getCanonicalFile();
        require(file.getPath().startsWith(root.getPath() + File.separator));
        return file;
    }
    private void writePhase(String next) throws Exception {
        JSONObject status = new JSONObject().put("schemaVersion", 1).put("runId", runId).put("phase", next);
        if ("failed".equals(next)) status.put("failedStage", phase);
        AtomicFile file = new AtomicFile(phaseFile);
        FileOutputStream output = null;
        try {
            output = file.startWrite();
            output.write(status.toString().getBytes(StandardCharsets.UTF_8));
            output.getFD().sync();
            file.finishWrite(output);
            output = null;
            Os.chmod(phaseFile.getAbsolutePath(), 0600);
        } finally { if (output != null) file.failWrite(output); }
    }
    private void requireEngineStopped() throws Exception {
        JSONObject status = new JSONObject(NativeCapture.status());
        require(!status.optBoolean("ok") && "Capture is not running".equals(status.optString("error")));
    }
    private boolean vpnReady() throws Exception {
        if (!CaptureVpnService.isActive() || !CaptureVpnService.isManaged() || CaptureVpnService.isStopping()) return false;
        JSONObject tunnel = new JSONObject(NativeCapture.tunnelStatus());
        JSONObject proxy = new JSONObject(NativeCapture.status());
        return tunnel.optBoolean("ok") && tunnel.getJSONObject("value").optBoolean("running")
                && proxy.optBoolean("ok") && proxy.getJSONObject("value").optBoolean("running");
    }
    private void grantVpnConsent() throws Exception {
        Intent consent = VpnService.prepare(context);
        Activity host = null;
        try {
            if (consent != null) {
                host = instrumentation.startActivitySync(new Intent(context, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
                final Activity activity = host;
                activity.runOnUiThread(() -> activity.startActivityForResult(consent, 49123));
                long deadline = SystemClock.elapsedRealtime() + 30000;
                boolean clicked = false;
                while (SystemClock.elapsedRealtime() < deadline) {
                    AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
                    AccessibilityNodeInfo button = CaptureSmokeInstrumentation.findConsentButton(root);
                    if (button != null && button.performAction(AccessibilityNodeInfo.ACTION_CLICK)) { clicked = true; break; }
                    Thread.sleep(100);
                }
                require(clicked);
            }
            long deadline = SystemClock.elapsedRealtime() + 10000;
            while (SystemClock.elapsedRealtime() < deadline && VpnService.prepare(context) != null) Thread.sleep(100);
            require(VpnService.prepare(context) == null);
        } finally {
            if (host != null) { final Activity activity = host; activity.runOnUiThread(activity::finish); }
        }
    }
    private void runProbe(JSONObject config, int httpPort, int udpPort, int dnsWirePort) throws Exception {
        CountDownLatch completed = new CountDownLatch(1);
        AtomicReference<Intent> reply = new AtomicReference<>();
        BroadcastReceiver receiver = new BroadcastReceiver() {
            @Override public void onReceive(Context unused, Intent result) {
                if (PROBE_RESULT.equals(result.getAction()) && runId.equals(result.getStringExtra("runId"))
                        && reply.compareAndSet(null, result)) completed.countDown();
            }
        };
        if (Build.VERSION.SDK_INT >= 33) context.registerReceiver(receiver, new IntentFilter(PROBE_RESULT), Context.RECEIVER_EXPORTED);
        else context.registerReceiver(receiver, new IntentFilter(PROBE_RESULT));
        try {
            Intent probe = new Intent(PROBE_ACTION).setComponent(new ComponentName("dev.httpcapture.probe", "dev.httpcapture.probe.ProbeReceiver"));
            probe.putExtra("runId", runId);
            probe.putExtra("host", config.getString("host"));
            probe.putExtra("httpPort", httpPort);
            probe.putExtra("udpPort", udpPort);
            probe.putExtra("dnsWirePort", dnsWirePort);
            probe.putExtra("expectedRequestBodyBase64", config.getString("expectedRequestBodyBase64"));
            probe.putExtra("expectedResponseBodyBase64", config.getString("expectedResponseBodyBase64"));
            context.sendBroadcast(probe);
            require(completed.await(60000, TimeUnit.MILLISECONDS));
            Intent result = reply.get();
            if (!result.getBooleanExtra("ok", false)) {
                String stage = result.getStringExtra("stage");
                for (String allowed : new String[] {"config", "uid-vpn", "http", "udp", "dns-wire", "virtual-dns"})
                    if (allowed.equals(stage)) phase = "probe-" + allowed;
                probeFailureType = "ProbeFailure";
                String type = result.getStringExtra("errorType");
                for (String allowed : new String[] {"SocketTimeoutException", "ConnectException", "SocketException", "IllegalStateException", "IllegalArgumentException", "EOFException", "SecurityException", "InterruptedException", "IOException"})
                    if (allowed.equals(type)) probeFailureType = allowed;
                require(false);
            }
            for (String field : new String[] {"vpnReady", "noProxyHint", "httpOk", "udpOk", "dnsWireOk", "virtualDnsOk", "ok"})
                require(result.getBooleanExtra(field, false));
        } finally { context.unregisterReceiver(receiver); }
    }
    private static int port(int port) { require(port >= 1024 && port <= 65535); return port; }
    private static void checkedBody(String value) {
        require(value != null && value.length() <= 90000);
        require(Base64.decode(value, Base64.DEFAULT).length <= 65536);
    }
    private static void require(boolean condition) { if (!condition) throw new AssertionError(); }
}
