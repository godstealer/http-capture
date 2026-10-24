package dev.httpcapture.probe;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.util.Log;

import java.io.InputStream;
import java.net.Socket;
import java.nio.charset.StandardCharsets;

/** Separate-UID raw TCP client used only by the emulator VPN smoke test. */
public final class ProbeReceiver extends BroadcastReceiver {
    @Override public void onReceive(Context context, Intent intent) {
        PendingResult pending = goAsync();
        new Thread(() -> {
            try (Socket socket = new Socket("10.0.2.2", 18080)) {
                socket.setSoTimeout(10_000);
                socket.getOutputStream().write(("GET /vpn-smoke HTTP/1.1\r\n"
                        + "Host: 10.0.2.2:18080\r\nConnection: close\r\n\r\n")
                        .getBytes(StandardCharsets.US_ASCII));
                InputStream response = socket.getInputStream();
                if (response.read() != 'H') throw new IllegalStateException("origin did not respond with HTTP");
                Log.i("HttpCaptureProbe", "raw TCP request reached origin");
            } catch (Exception error) {
                Log.e("HttpCaptureProbe", "raw TCP request failed", error);
            } finally {
                pending.finish();
            }
        }, "vpn-probe-client").start();
    }
}
