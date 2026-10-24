package dev.httpcapture.probe;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.util.Log;

import java.io.ByteArrayInputStream;
import java.io.InputStream;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import java.security.cert.CertificateFactory;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLParameters;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManagerFactory;

/** Separate-UID raw TCP client used only by the emulator VPN smoke test. */
public final class ProbeReceiver extends BroadcastReceiver {
    @Override public void onReceive(Context context, Intent intent) {
        PendingResult pending = goAsync();
        new Thread(() -> {
            try {
                if ("dev.httpcapture.probe.RUN_TLS".equals(intent.getAction())) {
                    runTls(intent.getStringExtra("caPem"));
                } else {
                    runHttp();
                }
            } catch (Exception error) {
                Log.e("HttpCaptureProbe", "raw TCP request failed", error);
            } finally {
                pending.finish();
            }
        }, "vpn-probe-client").start();
    }

    private static void runHttp() throws Exception {
        try (Socket socket = new Socket("10.0.2.2", 18080)) {
            socket.setSoTimeout(10_000);
            socket.getOutputStream().write(("GET /vpn-smoke HTTP/1.1\r\n"
                    + "Host: 10.0.2.2:18080\r\nConnection: close\r\n\r\n")
                    .getBytes(StandardCharsets.US_ASCII));
            InputStream response = socket.getInputStream();
            if (response.read() != 'H') throw new IllegalStateException("origin did not respond with HTTP");
            Log.i("HttpCaptureProbe", "raw TCP request reached origin");
        }
    }

    private static void runTls(String caPem) throws Exception {
        if (caPem == null || !caPem.contains("BEGIN CERTIFICATE")) {
            throw new IllegalArgumentException("missing capture CA");
        }
        KeyStore roots = KeyStore.getInstance(KeyStore.getDefaultType());
        roots.load(null);
        roots.setCertificateEntry("capture-test-ca", CertificateFactory.getInstance("X.509")
                .generateCertificate(new ByteArrayInputStream(caPem.getBytes(StandardCharsets.US_ASCII))));
        TrustManagerFactory trust = TrustManagerFactory.getInstance(
                TrustManagerFactory.getDefaultAlgorithm());
        trust.init(roots);
        SSLContext tls = SSLContext.getInstance("TLS");
        tls.init(null, trust.getTrustManagers(), null);
        try (SSLSocket socket = (SSLSocket) tls.getSocketFactory()
                .createSocket("10.0.2.2", 18081)) {
            socket.setSoTimeout(15_000);
            SSLParameters params = socket.getSSLParameters();
            params.setEndpointIdentificationAlgorithm("HTTPS");
            socket.setSSLParameters(params);
            socket.startHandshake();
            socket.getOutputStream().write(("GET /vpn-tls-smoke HTTP/1.1\r\n"
                    + "Host: 10.0.2.2:18081\r\nConnection: close\r\n\r\n")
                    .getBytes(StandardCharsets.US_ASCII));
            InputStream response = socket.getInputStream();
            byte[] head = new byte[12];
            int length = 0;
            while (length < head.length) {
                int count = response.read(head, length, head.length - length);
                if (count == -1) break;
                length += count;
            }
            if (length != head.length || !new String(head, StandardCharsets.US_ASCII)
                    .startsWith("HTTP/1.1 403")) {
                throw new IllegalStateException("intercepted TLS request was not blocked");
            }
            Log.i("HttpCaptureProbe", "raw VPN TLS request was intercepted");
        }
    }
}
