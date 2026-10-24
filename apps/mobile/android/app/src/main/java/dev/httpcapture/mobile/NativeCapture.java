package dev.httpcapture.mobile;

/** Thin JNI interface. Each call returns {"ok":boolean,"value"|"error":...}. */
public final class NativeCapture {
    static { System.loadLibrary("http_capture_android"); }
    private NativeCapture() {}
    public static native String start(String dataDir, int port);
    public static native String stop();
    public static native String recent(String dataDir, String cursor, String urlContains);
    public static native String exportSession(String dataDir, String path);
    public static native String exportConfiguration(String dataDir, String path, String optionsJson, String passphrase);
    public static native String exportConfigurationRecovery(String dataDir, String path, String passphrase);
    public static native String previewConfiguration(String dataDir, String path, String passphrase);
    public static native String importConfiguration(String dataDir, String path, String sectionsJson, String expectedRevision, String passphrase);
    public static native String metadata(String dataDir, String id);
    public static native String savedRequests(String dataDir, int offset, String query, String collectionId);
    public static native String savedCollections(String dataDir);
    public static native String changeSavedCollection(String dataDir, String changeJson);
    public static native String savedRequest(String dataDir, String id);
    public static native String deleteSavedRequest(String dataDir, String id);
    public static native String saveRequest(String dataDir, String requestJson);
    public static native String certificate(String dataDir);
    public static native String certificates(String dataDir);
    public static native String importCertificate(String dataDir, String importJson);
    public static native String changeCertificate(String dataDir, String changeJson);
    public static native String bodyPreview(String dataDir, String id, String variant, String revision, long offset);
    public static native String blocking(String dataDir);
    public static native String responseRewrite(String dataDir);
    public static native String configureResponseRewrite(String dataDir, String updateJson);
    public static native String status();
    public static native String configureBlocking(String dataDir, String configJson);
    public static native String sendRequest(String dataDir, String requestJson);
    public static native String startTunnel(int tunFd, boolean captureQuic);
    public static native String stopTunnel();
    public static native String tunnelStatus();
}
