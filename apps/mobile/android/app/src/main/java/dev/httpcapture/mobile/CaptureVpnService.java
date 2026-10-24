package dev.httpcapture.mobile;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Intent;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.ProxyInfo;
import android.net.VpnService;
import android.os.Process;
import android.system.OsConstants;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.ParcelFileDescriptor;
import android.util.Log;

import org.json.JSONObject;
import org.json.JSONArray;

import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.util.Arrays;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** Owns the device VPN and the capture engine independently of the Activity lifecycle. */
public final class CaptureVpnService extends VpnService {
    public static final String ACTION_START = "dev.httpcapture.mobile.START_VPN";
    public static final String ACTION_STOP = "dev.httpcapture.mobile.STOP_VPN";
    public static final String EXTRA_PORT = "port";
    public static final String EXTRA_ALLOWED_PACKAGES = "allowed_packages";
    public static final String EXTRA_CAPTURE_QUIC = "capture_quic";
    private static final String CHANNEL = "capture-vpn";
    private static final int NOTIFICATION_ID = 1001;
    private static volatile boolean active;
    private static volatile boolean managed;
    private static volatile boolean stopPending;
    private static volatile String message = "VPN 未启动";
    private static volatile CaptureVpnService currentService;

    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private final Handler main = new Handler(Looper.getMainLooper());
    private final Runnable monitor = new Runnable() {
        @Override public void run() {
            if (!active) return;
            worker.execute(() -> {
                try {
                    JSONObject state = new JSONObject(NativeCapture.tunnelStatus());
                    if (!state.optBoolean("ok") && !underlyingNetworkLost && !networkRestartPending) {
                        message = "VPN 转发已退出：" + state.optString("error");
                        main.post(CaptureVpnService.this::stopSelf);
                    }
                } catch (Exception error) {
                    if (!underlyingNetworkLost && !networkRestartPending) {
                        message = "VPN 状态读取失败：" + error.getMessage();
                        main.post(CaptureVpnService.this::stopSelf);
                    }
                }
            });
            main.postDelayed(this, 1500);
        }
    };
    private ParcelFileDescriptor tunnel;
    private ConnectivityManager connectivity;
    private ConnectivityManager.NetworkCallback networkCallback;
    private Network underlyingNetwork;
    private volatile boolean underlyingNetworkLost;
    private volatile boolean networkRestartPending;
    private volatile int networkGeneration;
    private volatile boolean destroyed;
    private boolean shutdownStarted;
    private boolean captureQuic;

    public static boolean isActive() { return active; }
    public static boolean isManaged() { return managed; }
    public static boolean isStopping() { return stopPending; }
    public static String message() { return message; }
    public static void requestStop() { stopPending = true; }

    /** Called only for a TUN TCP tuple while this VPN owns the system tunnel. */
    public static String lookupConnectionOwner(String sourceAddress, int sourcePort,
                                                String destinationAddress, int destinationPort) {
        CaptureVpnService service = currentService;
        if (Build.VERSION.SDK_INT < 29 || service == null || !managed || service.destroyed) return "";
        try {
            ConnectivityManager manager = service.getSystemService(ConnectivityManager.class);
            if (manager == null) return "";
            int uid = manager.getConnectionOwnerUid(OsConstants.IPPROTO_TCP,
                new InetSocketAddress(InetAddress.getByName(sourceAddress), sourcePort),
                new InetSocketAddress(InetAddress.getByName(destinationAddress), destinationPort));
            if (uid == Process.INVALID_UID || uid < 0) return "";
            String[] packages = service.getPackageManager().getPackagesForUid(uid);
            if (packages == null) packages = new String[0];
            Arrays.sort(packages);
            JSONArray names = new JSONArray();
            for (int index = 0; index < Math.min(packages.length, 16); index++)
                names.put(packages[index].substring(0, Math.min(packages[index].length(), 256)));
            JSONObject result = new JSONObject().put("uid", uid).put("packages", names);
            if (packages.length == 1) {
                try {
                    CharSequence label = service.getPackageManager().getApplicationLabel(
                        service.getPackageManager().getApplicationInfo(packages[0], 0));
                    String text = label.toString();
                    result.put("label", text.substring(0, Math.min(text.length(), 128)));
                } catch (android.content.pm.PackageManager.NameNotFoundException ignored) { }
            }
            return result.toString();
        } catch (Exception error) {
            return "";
        }
    }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent == null || ACTION_STOP.equals(intent.getAction())) {
            beginShutdown();
            stopSelf();
            return START_NOT_STICKY;
        }
        if (!ACTION_START.equals(intent.getAction())) return START_NOT_STICKY;
        if (managed || stopPending) return START_NOT_STICKY;
        shutdownStarted = false;
        destroyed = false;
        int port = intent.getIntExtra(EXTRA_PORT, 0);
        captureQuic = intent.getBooleanExtra(EXTRA_CAPTURE_QUIC, false);
        String[] allowedPackages = intent.getStringArrayExtra(EXTRA_ALLOWED_PACKAGES);
        if (port < 1 || port > 65535) {
            message = "VPN 端口无效";
            stopSelf();
            return START_NOT_STICKY;
        }
        startForeground(NOTIFICATION_ID, notification());
        managed = true;
        stopPending = false;
        message = "正在建立 VPN…";
        worker.execute(() -> connect(port, allowedPackages));
        return START_NOT_STICKY;
    }

    private void connect(int port, String[] allowedPackages) {
        try {
            JSONObject proxy = new JSONObject(NativeCapture.start(getFilesDir().getAbsolutePath(), port));
            if (!proxy.optBoolean("ok")) throw new IllegalStateException(proxy.optString("error"));
            Builder builder = new Builder()
                .setSession("HTTP Capture")
                .setMtu(1500)
                .addAddress("10.111.0.2", 32)
                .addRoute("0.0.0.0", 0)
                .addAddress("fd00:198:18::2", 128)
                .addRoute("::", 0)
                .addDnsServer("10.111.0.1");
            if (allowedPackages == null || allowedPackages.length == 0) {
                builder.addDisallowedApplication(getPackageName());
            } else {
                int added = 0;
                for (String packageName : allowedPackages) {
                    if (packageName == null || packageName.equals(getPackageName())) continue;
                    try {
                        builder.addAllowedApplication(packageName);
                        added++;
                    } catch (android.content.pm.PackageManager.NameNotFoundException error) {
                        throw new IllegalStateException("抓包应用未安装或系统未允许查询：" + packageName, error);
                    }
                }
                if (added == 0) throw new IllegalStateException("选中的应用已卸载，请重新选择");
            }
            if (Build.VERSION.SDK_INT >= 29) {
                builder.setHttpProxy(ProxyInfo.buildDirectProxy("127.0.0.1", port));
            }
            tunnel = builder.establish();
            if (tunnel == null) throw new IllegalStateException("系统未建立 VPN；请重新授权");
            currentService = this;
            JSONObject started = new JSONObject(NativeCapture.startTunnel(tunnel.getFd(), captureQuic));
            if (!started.optBoolean("ok")) throw new IllegalStateException(started.optString("error"));
            if (destroyed) throw new IllegalStateException("VPN 服务已停止");
            active = true;
            message = "VPN 抓包运行中 · 127.0.0.1:" + port;
            main.post(this::watchUnderlyingNetworks);
            main.post(monitor);
        } catch (Exception error) {
            active = false;
            message = "VPN 启动失败：" + error.getMessage();
            requestStop();
            stopSelf();
        }
    }

    private void watchUnderlyingNetworks() {
        if (destroyed || networkCallback != null) return;
        connectivity = getSystemService(ConnectivityManager.class);
        if (connectivity == null) return;
        networkCallback = new ConnectivityManager.NetworkCallback() {
            // Android 8+ follows onAvailable with ordered capabilities. Reading
            // getNetworkCapabilities here can return stale data during a handoff.
            @Override public void onAvailable(Network network) { }
            @Override public void onLost(Network network) {
                if (destroyed || !network.equals(underlyingNetwork)) return;
                underlyingNetwork = null;
                underlyingNetworkLost = true;
                networkRestartPending = true;
                ++networkGeneration;
                if (!setUnderlyingNetworks(null)) {
                    Log.w("HttpCaptureVpn", "Android did not clear the lost underlying network");
                }
                if (active) message = "网络已断开，等待连接…";
            }
            @Override public void onCapabilitiesChanged(Network network, NetworkCapabilities capabilities) {
                updateUnderlyingNetwork(network, capabilities);
            }
        };
        try {
            connectivity.registerDefaultNetworkCallback(networkCallback, main);
            Network initial = connectivity.getActiveNetwork();
            if (initial != null) updateUnderlyingNetwork(initial, connectivity.getNetworkCapabilities(initial));
        } catch (RuntimeException error) {
            Log.w("HttpCaptureVpn", "Could not watch underlying networks", error);
            networkCallback = null;
        }
    }

    private void updateUnderlyingNetwork(Network network, NetworkCapabilities capabilities) {
        if (destroyed || tunnel == null || connectivity == null || network == null) return;
        try {
            if (capabilities == null || !capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                    || !capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)) return;
            if (!setUnderlyingNetworks(new Network[] {network})) {
                Log.w("HttpCaptureVpn", "Android did not accept the underlying network update");
            }
            if (network.equals(underlyingNetwork)) return;
            boolean switched = underlyingNetwork != null || underlyingNetworkLost;
            if (switched && active) networkRestartPending = true;
            underlyingNetwork = network;
            underlyingNetworkLost = false;
            int generation = ++networkGeneration;
            if (switched && active) {
                message = "网络已切换，正在重建连接…";
                worker.execute(() -> restartConnections(generation));
            }
        } catch (RuntimeException error) {
            Log.w("HttpCaptureVpn", "Could not update underlying networks", error);
        }
    }

    private void restartConnections(int generation) {
        if (destroyed || !active || generation != networkGeneration || tunnel == null) return;
        try {
            JSONObject stopped = new JSONObject(NativeCapture.stopTunnel());
            if (!stopped.optBoolean("ok")) throw new IllegalStateException(stopped.optString("error"));
            // A newer handoff (or complete network loss) supersedes this restart.
            // The next valid network callback will queue its own restart.
            if (destroyed || !active || generation != networkGeneration || underlyingNetworkLost) return;
            JSONObject started = new JSONObject(NativeCapture.startTunnel(tunnel.getFd(), captureQuic));
            if (!started.optBoolean("ok")) throw new IllegalStateException(started.optString("error"));
            if (generation == networkGeneration && !underlyingNetworkLost) {
                networkRestartPending = false;
                message = "VPN 抓包运行中 · 网络连接已更新";
            }
        } catch (Exception error) {
            networkRestartPending = false;
            active = false;
            message = "切换网络后重建连接失败：" + error.getMessage();
            requestStop();
            main.post(this::stopSelf);
        }
    }

    private Notification notification() {
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel(
            CHANNEL, "HTTP Capture VPN", NotificationManager.IMPORTANCE_LOW));
        Intent open = new Intent(this, MainActivity.class);
        PendingIntent pending = PendingIntent.getActivity(
            this, 0, open, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Intent stop = new Intent(this, CaptureVpnService.class).setAction(ACTION_STOP);
        PendingIntent stopPendingIntent = PendingIntent.getService(
            this, 1, stop, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        return new Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentTitle("HTTP Capture 正在抓取流量")
            .setContentText("点击打开抓包界面")
            .setContentIntent(pending)
            .addAction(android.R.drawable.ic_media_pause, "停止抓包", stopPendingIntent)
            .setOngoing(true)
            .build();
    }

    @Override public void onRevoke() {
        message = "VPN 权限已撤销";
        beginShutdown();
        stopSelf();
    }

    @Override public void onDestroy() {
        beginShutdown();
        worker.shutdown();
        super.onDestroy();
    }

    private void beginShutdown() {
        if (shutdownStarted) return;
        shutdownStarted = true;
        destroyed = true;
        if (currentService == this) currentService = null;
        active = false;
        requestStop();
        main.removeCallbacks(monitor);
        if (networkCallback != null && connectivity != null) {
            try { connectivity.unregisterNetworkCallback(networkCallback); }
            catch (RuntimeException error) { Log.w("HttpCaptureVpn", "Could not remove network callback", error); }
            networkCallback = null;
        }
        worker.execute(() -> {
            try {
                if (!message.startsWith("VPN 权限已撤销")) message = "正在停止 VPN 隧道…";
                NativeCapture.stopTunnel();
                if (tunnel != null) {
                    try { tunnel.close(); } catch (IOException ignored) { }
                    tunnel = null;
                }
                if (!message.startsWith("VPN 权限已撤销")) message = "正在停止抓包引擎…";
                NativeCapture.stop();
                if (!message.startsWith("VPN 启动失败") && !message.startsWith("VPN 转发已退出")
                        && !message.startsWith("VPN 状态读取失败")
                        && !message.startsWith("切换网络后重建连接失败")
                        && !message.startsWith("VPN 权限已撤销")) message = "VPN 已停止";
            } finally {
                managed = false;
                stopPending = false;
                stopForeground(STOP_FOREGROUND_REMOVE);
            }
        });
    }
}
