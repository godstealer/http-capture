package dev.httpcapture.mobile;

/** Thin JNI interface. Each call returns {"ok":boolean,"value"|"error":...}. */
public final class NativeCapture {
    static { System.loadLibrary("http_capture_android"); }
    private NativeCapture() {}
    public static native String start(String dataDir, int port);
    public static native String stop();
    public static native String recent(String cursor, String urlContains);
    public static native String metadata(String id);
    public static native String savedRequests(int offset, String query, String collectionId);
    public static native String savedCollections();
    public static native String changeSavedCollection(String changeJson);
    public static native String savedRequest(String id);
    public static native String deleteSavedRequest(String id);
    public static native String saveRequest(String requestJson);
    public static native String certificate();
    public static native String certificates();
    public static native String importCertificate(String importJson);
    public static native String changeCertificate(String changeJson);
    public static native String bodyPreview(String id, String variant, String revision, long offset);
    public static native String blocking();
    public static native String responseRewrite();
    public static native String configureResponseRewrite(String updateJson);
    public static native String status();
    public static native String configureBlocking(String configJson);
    public static native String sendRequest(String requestJson);
    public static native String startTunnel(int tunFd, boolean captureQuic);
    public static native String stopTunnel();
    public static native String tunnelStatus();
}
