package dev.httpcapture.mobile;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.database.Cursor;
import android.net.VpnService;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.text.InputType;
import android.provider.OpenableColumns;
import android.util.Base64;
import android.graphics.Typeface;
import android.view.Gravity;
import android.view.View;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.ArrayAdapter;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Spinner;
import android.widget.TextView;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;
import org.json.JSONTokener;

import java.io.OutputStream;
import java.io.InputStream;
import java.io.ByteArrayOutputStream;
import java.net.URI;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.charset.CodingErrorAction;
import java.text.Collator;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Locale;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** First device UI for the shared Rust capture engine. */
public final class MainActivity extends Activity {
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private final Handler main = new Handler(Looper.getMainLooper());
    private EditText port;
    private Button start;
    private Button stop;
    private Button exportCa;
    private Button blockingRules;
    private Button responseRewriteRules;
    private Button manualRequest;
    private Button selectApps;
    private Button addApp;
    private CheckBox captureQuic;
    private Button previousPage;
    private Button nextPage;
    private EditText urlSearch;
    private TextView pageInfo;
    private TextView status;
    private LinearLayout results;
    private ScrollView resultScroll;
    private String lastRowsJson = "";
    private String pageCursor = "";
    private String nextCursor = "";
    private String urlFilter = "";
    private final List<String> pageHistory = new ArrayList<>();
    private boolean pageLoading;
    private boolean running;
    private boolean pollPending;
    private boolean vpnRequested;
    private boolean vpnWasManaged;
    private boolean vpnPermissionPending;
    private int pendingVpnPort;
    private String certificatePem;
    private final Set<String> selectedApps = new HashSet<>();
    private static final int SAVE_CA_REQUEST = 1201;
    private static final int VPN_REQUEST = 1202;
    private static final int PICK_REQUEST_FILE = 1203;
    private static final String[] BODY_MODES = {
        "文本", "JSON", "URL 表单", "文件", "Multipart 表单", "Base64"
    };
    private static final String[] SEND_ENGINES = {"auto", "native", "h2", "h3"};
    private static final String[] SEND_ENGINE_LABELS = {
        "自动协商（HTTP/1.1 或 HTTP/2）", "HTTP/1.1", "HTTP/2", "HTTP/3"
    };
    private ManualRequestDraft pendingFileRequest;

    private static final class ManualRequestDraft {
        String method = "GET";
        String url = "";
        String headers = "";
        String body = "";
        String engine = "auto";
        int bodyMode;
        Uri fileUri;
        String fileName;
    }

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        if (state != null) {
            vpnPermissionPending = state.getBoolean("vpnPermissionPending", false);
            pendingVpnPort = state.getInt("pendingVpnPort", 0);
            if (state.getBoolean("requestFilePending", false)) {
                pendingFileRequest = new ManualRequestDraft();
                pendingFileRequest.method = state.getString("requestMethod", "GET");
                pendingFileRequest.url = state.getString("requestUrl", "");
                pendingFileRequest.headers = state.getString("requestHeaders", "");
                pendingFileRequest.body = state.getString("requestBody", "");
                pendingFileRequest.engine = state.getString("requestEngine", "auto");
                pendingFileRequest.bodyMode = state.getInt("requestBodyMode", 0);
                String fileUri = state.getString("requestFileUri");
                if (fileUri != null) pendingFileRequest.fileUri = Uri.parse(fileUri);
                pendingFileRequest.fileName = state.getString("requestFileName");
            }
        }
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        int inset = (int) (16 * getResources().getDisplayMetrics().density);
        layout.setPadding(inset, inset, inset, inset);

        TextView title = new TextView(this);
        title.setText("HTTP Capture · Android");
        title.setTextSize(22);
        layout.addView(title);

        status = new TextView(this);
        status.setText("代理未启动");
        layout.addView(status);

        port = new EditText(this);
        port.setHint("本机代理端口");
        port.setInputType(InputType.TYPE_CLASS_NUMBER);
        port.setSingleLine(true);
        port.setText(getPreferences(MODE_PRIVATE).getString("port", "8888"));
        layout.addView(port);

        Set<String> savedApps = getPreferences(MODE_PRIVATE).getStringSet("capture_apps", null);
        if (savedApps != null) selectedApps.addAll(savedApps);
        selectApps = button(layout, "", view -> chooseApps());
        updateAppSelectionLabel();
        addApp = button(layout, "按包名添加应用", view -> addAppByPackageName());

        captureQuic = new CheckBox(this);
        captureQuic.setText("尝试抓取 HTTP/3（UDP/443，需信任 CA）");
        captureQuic.setChecked(getPreferences(MODE_PRIVATE).getBoolean("capture_quic", false));
        captureQuic.setOnCheckedChangeListener((button, checked) ->
            getPreferences(MODE_PRIVATE).edit().putBoolean("capture_quic", checked).apply());
        layout.addView(captureQuic);

        start = button(layout, "启动 VPN 抓包", view -> startCapture());
        stop = button(layout, "停止", view -> stopCapture());
        exportCa = button(layout, "保存 CA 公钥证书", view -> saveCertificate());
        blockingRules = button(layout, "阻止规则", view -> loadBlocking());
        responseRewriteRules = button(layout, "响应修改规则", view -> loadResponseRewrite());
        manualRequest = button(layout, "新建请求", view -> openManualRequest());
        stop.setEnabled(false);
        exportCa.setEnabled(false);
        blockingRules.setEnabled(false);
        responseRewriteRules.setEnabled(false);
        manualRequest.setEnabled(false);

        TextView hint = new TextView(this);
        hint.setText("首次启动需要授权系统 VPN；HTTPS 正文还需要目标 App 信任本 CA。普通 UDP 直连且不记录；开启 HTTP/3 后，UDP/443 会尝试进入本机解密入口，不支持 ECH/隐藏 SNI。");
        layout.addView(hint);

        LinearLayout searchRow = new LinearLayout(this);
        searchRow.setOrientation(LinearLayout.HORIZONTAL);
        urlSearch = new EditText(this);
        urlSearch.setHint("按 URL 搜索");
        urlSearch.setSingleLine(true);
        urlFilter = getPreferences(MODE_PRIVATE).getString("url_filter", "");
        urlSearch.setText(urlFilter);
        searchRow.addView(urlSearch, new LinearLayout.LayoutParams(0, -2, 1));
        button(searchRow, "搜索", view -> applyUrlFilter(urlSearch.getText().toString()));
        button(searchRow, "清除", view -> {
            urlSearch.setText("");
            applyUrlFilter("");
        });
        layout.addView(searchRow);

        LinearLayout navigation = new LinearLayout(this);
        navigation.setOrientation(LinearLayout.HORIZONTAL);
        previousPage = button(navigation, "上一页", view -> loadPreviousPage());
        pageInfo = new TextView(this);
        pageInfo.setGravity(Gravity.CENTER);
        navigation.addView(pageInfo, new LinearLayout.LayoutParams(0, -2, 1));
        nextPage = button(navigation, "下一页", view -> loadNextPage());
        layout.addView(navigation);
        updatePagingControls();

        results = new LinearLayout(this);
        results.setOrientation(LinearLayout.VERTICAL);
        resultScroll = new ScrollView(this);
        resultScroll.setFillViewport(true);
        layout.addView(results);
        resultScroll.addView(layout);
        showEmptyResults();
        setContentView(resultScroll);
        if (CaptureVpnService.isStopping()) {
            status.setText("正在停止 VPN…");
            start.setEnabled(false);
            setAppSelectionEnabled(false);
            waitForVpnStop();
        } else if (CaptureVpnService.isManaged()) {
            running = true;
            vpnRequested = true;
            vpnWasManaged = true;
            start.setEnabled(false);
            stop.setEnabled(true);
            exportCa.setEnabled(true);
            blockingRules.setEnabled(true);
            responseRewriteRules.setEnabled(true);
            manualRequest.setEnabled(true);
            setAppSelectionEnabled(false);
            updatePagingControls();
            status.setText(CaptureVpnService.message());
            poll();
        } else {
            restoreStandaloneProxy();
        }
    }

    private void restoreStandaloneProxy() {
        start.setEnabled(false);
        setAppSelectionEnabled(false);
        worker.execute(() -> {
            JSONObject reply = parseReply(NativeCapture.status());
            main.post(() -> {
                if (CaptureVpnService.isManaged() || CaptureVpnService.isStopping() || vpnRequested) return;
                JSONObject value = reply.optJSONObject("value");
                if (reply.optBoolean("ok") && value != null && value.optBoolean("running")) {
                    running = true;
                    vpnRequested = false;
                    stop.setEnabled(true);
                    exportCa.setEnabled(true);
                    blockingRules.setEnabled(true);
                    responseRewriteRules.setEnabled(true);
                    manualRequest.setEnabled(true);
                    status.setText("本机代理运行中 · " + value.optString("address"));
                    updatePagingControls();
                    poll();
                } else {
                    start.setEnabled(true);
                    setAppSelectionEnabled(true);
                }
            });
        });
    }

    private Button button(LinearLayout layout, String text, View.OnClickListener action) {
        Button button = new Button(this);
        button.setText(text);
        button.setOnClickListener(action);
        layout.addView(button);
        return button;
    }

    private void setAppSelectionEnabled(boolean enabled) {
        selectApps.setEnabled(enabled);
        addApp.setEnabled(enabled);
        captureQuic.setEnabled(enabled);
    }

    private void updatePagingControls() {
        previousPage.setEnabled(running && !pageLoading && !pageHistory.isEmpty());
        nextPage.setEnabled(running && !pageLoading && !nextCursor.isEmpty());
        pageInfo.setText("第 " + (pageHistory.size() + 1) + " 页");
    }

    private void applyUrlFilter(String input) {
        String filter = input.trim();
        if (filter.getBytes(StandardCharsets.UTF_8).length > 2048) {
            status.setText("URL 搜索内容不能超过 2048 字节");
            return;
        }
        if (filter.equals(urlFilter)) return;
        urlFilter = filter;
        getPreferences(MODE_PRIVATE).edit().putString("url_filter", filter).apply();
        pageCursor = "";
        nextCursor = "";
        pageHistory.clear();
        lastRowsJson = "";
        if (running) loadSelectedPage();
        else updatePagingControls();
    }

    private void loadNextPage() {
        if (!running || pageLoading || nextCursor.isEmpty()) return;
        pageHistory.add(pageCursor);
        pageCursor = nextCursor;
        nextCursor = "";
        loadSelectedPage();
    }

    private void loadPreviousPage() {
        if (!running || pageLoading || pageHistory.isEmpty()) return;
        pageCursor = pageHistory.remove(pageHistory.size() - 1);
        nextCursor = "";
        loadSelectedPage();
    }

    private void loadSelectedPage() {
        pageLoading = true;
        lastRowsJson = "";
        updatePagingControls();
        final String cursor = pageCursor;
        final String filter = urlFilter;
        worker.execute(() -> {
            JSONObject reply = parseReply(NativeCapture.recent(cursor, filter));
            main.post(() -> {
                pageLoading = false;
                if (!running || !cursor.equals(pageCursor) || !filter.equals(urlFilter)) return;
                if (reply.optBoolean("ok")) {
                    JSONObject value = reply.optJSONObject("value");
                    if (value != null) {
                        showRecent(value);
                        resultScroll.scrollTo(0, 0);
                    }
                } else {
                    status.setText("读取历史失败：" + reply.optString("error"));
                    results.removeAllViews();
                    showEmptyResults();
                }
                updatePagingControls();
            });
        });
    }

    private void updateAppSelectionLabel() {
        selectApps.setText(selectedApps.isEmpty()
            ? "抓包应用：全部应用（点此选择）"
            : "抓包应用：已选 " + selectedApps.size() + " 个（点此修改）");
    }

    private void chooseApps() {
        PackageManager manager = getPackageManager();
        Intent launcher = new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER);
        List<ResolveInfo> launchers = manager.queryIntentActivities(launcher, 0);
        Map<String, String> available = new LinkedHashMap<>();
        for (ResolveInfo info : launchers) {
            if (info.activityInfo == null) continue;
            String packageName = info.activityInfo.packageName;
            if (getPackageName().equals(packageName)) continue;
            CharSequence label = info.loadLabel(manager);
            available.putIfAbsent(packageName,
                (label == null ? packageName : label.toString()) + " · " + packageName);
        }
        for (String packageName : selectedApps) {
            available.putIfAbsent(packageName, packageName + " · 无桌面图标或已卸载");
        }
        List<Map.Entry<String, String>> apps = new ArrayList<>(available.entrySet());
        Collator collator = Collator.getInstance();
        apps.sort(Comparator.comparing(Map.Entry::getValue, collator));
        String[] labels = new String[apps.size()];
        boolean[] checked = new boolean[apps.size()];
        Set<String> draft = new HashSet<>(selectedApps);
        draft.retainAll(available.keySet());
        for (int i = 0; i < apps.size(); i++) {
            labels[i] = apps.get(i).getValue();
            checked[i] = draft.contains(apps.get(i).getKey());
        }
        new AlertDialog.Builder(this)
            .setTitle("选择需要抓包的应用")
            .setMultiChoiceItems(labels, checked, (dialog, index, selected) -> {
                String packageName = apps.get(index).getKey();
                if (selected) draft.add(packageName);
                else draft.remove(packageName);
            })
            .setNeutralButton("全部应用", (dialog, which) -> saveAppSelection(new HashSet<>()))
            .setPositiveButton("保存选择", (dialog, which) -> saveAppSelection(draft))
            .setNegativeButton("取消", null)
            .show();
    }

    private void addAppByPackageName() {
        EditText input = new EditText(this);
        input.setSingleLine(true);
        input.setHint("例如 com.example.app");
        int padding = (int) (20 * getResources().getDisplayMetrics().density);
        input.setPadding(padding, input.getPaddingTop(), padding, input.getPaddingBottom());
        AlertDialog dialog = new AlertDialog.Builder(this)
            .setTitle("添加没有桌面图标的应用")
            .setMessage("填写应用包名。启动 VPN 时会检查应用是否已安装。")
            .setView(input)
            .setPositiveButton("添加", null)
            .setNegativeButton("取消", null)
            .create();
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
                String packageName = input.getText().toString().trim();
                if (!packageName.matches("[A-Za-z_][A-Za-z0-9_]*(\\.[A-Za-z_][A-Za-z0-9_]*)+")
                        || packageName.equals(getPackageName())) {
                    input.setError("请输入类似 com.example.app 的应用包名");
                    return;
                }
                Set<String> updated = new HashSet<>(selectedApps);
                updated.add(packageName);
                saveAppSelection(updated);
                dialog.dismiss();
            }));
        dialog.show();
    }

    private void saveAppSelection(Set<String> packages) {
        selectedApps.clear();
        selectedApps.addAll(packages);
        getPreferences(MODE_PRIVATE).edit()
            .putStringSet("capture_apps", new HashSet<>(selectedApps)).apply();
        updateAppSelectionLabel();
    }

    private void startCapture() {
        final int selectedPort;
        try {
            selectedPort = Integer.parseInt(port.getText().toString().trim());
            if (selectedPort < 1 || selectedPort > 65535) throw new NumberFormatException();
        } catch (NumberFormatException error) {
            status.setText("端口必须为 1–65535");
            return;
        }
        start.setEnabled(false);
        setAppSelectionEnabled(false);
        status.setText("正在启动…");
        worker.execute(() -> {
            JSONObject reply = parseReply(NativeCapture.start(getFilesDir().getAbsolutePath(), selectedPort));
            main.post(() -> {
                if (reply.optBoolean("ok")) {
                    getPreferences(MODE_PRIVATE).edit().putString("port", String.valueOf(selectedPort)).apply();
                    running = true;
                    pageCursor = "";
                    nextCursor = "";
                    pageHistory.clear();
                    lastRowsJson = "";
                    results.removeAllViews();
                    showEmptyResults();
                    status.setText("代理运行中 · 127.0.0.1:" + selectedPort);
                    stop.setEnabled(true);
                    exportCa.setEnabled(true);
                    blockingRules.setEnabled(true);
                    responseRewriteRules.setEnabled(true);
                    manualRequest.setEnabled(true);
                    updatePagingControls();
                    requestVpn(selectedPort);
                    poll();
                } else {
                    status.setText("启动失败：" + reply.optString("error", "未知错误"));
                    start.setEnabled(true);
                    setAppSelectionEnabled(true);
                }
            });
        });
    }

    private void requestVpn(int selectedPort) {
        pendingVpnPort = selectedPort;
        Intent permission = VpnService.prepare(this);
        if (permission == null) {
            vpnPermissionPending = false;
            startVpnService(selectedPort);
        } else {
            vpnPermissionPending = true;
            status.setText("请授权系统 VPN 以接管手机流量");
            startActivityForResult(permission, VPN_REQUEST);
        }
    }

    private void startVpnService(int selectedPort) {
        if (selectedPort < 1 || selectedPort > 65535) {
            status.setText("VPN 端口无效，请重新启动");
            return;
        }
        vpnPermissionPending = false;
        pendingVpnPort = 0;
        vpnRequested = true;
        vpnWasManaged = false;
        Intent intent = new Intent(this, CaptureVpnService.class);
        intent.setAction(CaptureVpnService.ACTION_START);
        intent.putExtra(CaptureVpnService.EXTRA_PORT, selectedPort);
        intent.putExtra(CaptureVpnService.EXTRA_ALLOWED_PACKAGES,
            selectedApps.toArray(new String[0]));
        intent.putExtra(CaptureVpnService.EXTRA_CAPTURE_QUIC, captureQuic.isChecked());
        if (Build.VERSION.SDK_INT >= 26) startForegroundService(intent);
        else startService(intent);
        running = true;
        start.setEnabled(false);
        stop.setEnabled(true);
        setAppSelectionEnabled(false);
        exportCa.setEnabled(true);
        blockingRules.setEnabled(true);
        responseRewriteRules.setEnabled(true);
        manualRequest.setEnabled(true);
        status.setText("正在建立 VPN…");
        poll();
    }

    private void stopCapture() {
        vpnPermissionPending = false;
        pendingVpnPort = 0;
        stop.setEnabled(false);
        status.setText("正在停止…");
        if (vpnRequested) {
            CaptureVpnService.requestStop();
            Intent intent = new Intent(this, CaptureVpnService.class);
            intent.setAction(CaptureVpnService.ACTION_STOP);
            startService(intent);
            vpnRequested = false;
            vpnWasManaged = false;
            running = false;
            updatePagingControls();
            start.setEnabled(false);
            setAppSelectionEnabled(false);
            exportCa.setEnabled(false);
            blockingRules.setEnabled(false);
            responseRewriteRules.setEnabled(false);
            manualRequest.setEnabled(false);
            status.setText("正在停止 VPN…");
            waitForVpnStop();
            return;
        }
        worker.execute(() -> {
            JSONObject reply = parseReply(NativeCapture.stop());
            main.post(() -> {
                running = false;
                updatePagingControls();
                status.setText(reply.optBoolean("ok") ? "代理已停止" : "停止失败：" + reply.optString("error"));
                start.setEnabled(true);
                setAppSelectionEnabled(true);
                exportCa.setEnabled(false);
                blockingRules.setEnabled(false);
                responseRewriteRules.setEnabled(false);
                manualRequest.setEnabled(false);
            });
        });
    }

    private void waitForVpnStop() {
        if (CaptureVpnService.isManaged() || CaptureVpnService.isStopping()) {
            main.postDelayed(this::waitForVpnStop, 250);
            return;
        }
        status.setText(CaptureVpnService.message());
        start.setEnabled(true);
        setAppSelectionEnabled(true);
        stop.setEnabled(false);
        exportCa.setEnabled(false);
        blockingRules.setEnabled(false);
        responseRewriteRules.setEnabled(false);
        manualRequest.setEnabled(false);
        updatePagingControls();
    }

    private void poll() {
        if (!running || pollPending) return;
        pollPending = true;
        final String cursor = pageCursor;
        final String filter = urlFilter;
        worker.execute(() -> {
            JSONObject reply = parseReply(NativeCapture.recent(cursor, filter));
            main.post(() -> {
                pollPending = false;
                if (!running) return;
                if (vpnRequested) {
                    boolean managed = CaptureVpnService.isManaged();
                    if (managed) vpnWasManaged = true;
                    String vpnMessage = CaptureVpnService.message();
                    status.setText(vpnMessage);
                    if (!managed && (vpnWasManaged
                            || vpnMessage.startsWith("VPN 启动失败")
                            || vpnMessage.startsWith("VPN 转发已退出")
                            || vpnMessage.startsWith("VPN 状态读取失败")
                            || vpnMessage.startsWith("VPN 权限已撤销"))) {
                        running = false;
                        updatePagingControls();
                        vpnRequested = false;
                        vpnWasManaged = false;
                        start.setEnabled(false);
                        setAppSelectionEnabled(false);
                        stop.setEnabled(false);
                        exportCa.setEnabled(false);
                        blockingRules.setEnabled(false);
                        responseRewriteRules.setEnabled(false);
                        manualRequest.setEnabled(false);
                        waitForVpnStop();
                        return;
                    }
                }
                if (!cursor.equals(pageCursor) || !filter.equals(urlFilter)) {
                    main.postDelayed(this::poll, 1500);
                    return;
                }
                if (pageLoading) {
                    main.postDelayed(this::poll, 1500);
                    return;
                }
                if (reply.optBoolean("ok")) {
                    JSONObject value = reply.optJSONObject("value");
                    if (value != null) showRecent(value);
                } else {
                    status.setText("读取流量失败：" + reply.optString("error"));
                }
                main.postDelayed(this::poll, 1500);
            });
        });
    }

    private void showRecent(JSONObject value) {
        JSONArray rows = value.optJSONArray("requests");
        if (rows == null) return;
        nextCursor = value.isNull("nextCursor") ? "" : value.optString("nextCursor");
        updatePagingControls();
        String snapshot = value.toString();
        if (snapshot.equals(lastRowsJson)) return;
        lastRowsJson = snapshot;
        results.removeAllViews();
        for (int i = 0; i < rows.length(); i++) {
            JSONObject summary = rows.optJSONObject(i);
            if (summary == null) continue;
            String id = summary.optString("id");
            String responseStatus = String.valueOf(summary.optInt("status", 0));
            if ("0".equals(responseStatus)) responseStatus = "";
            TextView row = new TextView(this);
            row.setTextSize(14);
            row.setPadding(12, 16, 12, 16);
            row.setGravity(Gravity.START);
            row.setClickable(true);
            row.setOnClickListener(view -> showFlowDetail(id));
            row.setBackgroundResource(android.R.drawable.list_selector_background);
            row.setText(summary.optString("method", "?")
                + (responseStatus.isEmpty() ? "" : "  " + responseStatus) + "\n"
                + summary.optString("url", ""));
            results.addView(row, new LinearLayout.LayoutParams(-1, -2));
        }
        if (results.getChildCount() == 0) showEmptyResults();
    }

    private void showEmptyResults() {
        TextView empty = new TextView(this);
        empty.setText(urlFilter.isEmpty() ? "暂无流量" : "没有匹配的流量");
        empty.setPadding(12, 20, 12, 20);
        results.addView(empty);
    }

    private void showFlowDetail(String id) {
        worker.execute(() -> {
            JSONObject reply = parseReply(NativeCapture.metadata(id));
            main.post(() -> {
                if (!reply.optBoolean("ok")) {
                    status.setText("读取详情失败：" + reply.optString("error"));
                    return;
                }
                JSONObject flow = reply.optJSONObject("value");
                if (flow != null) showFlowDialog(flow);
            });
        });
    }

    private void showFlowDialog(JSONObject flow) {
        LinearLayout contentLayout = new LinearLayout(this);
        contentLayout.setOrientation(LinearLayout.VERTICAL);
        Button edit = new Button(this);
        edit.setText("复制到新请求");
        contentLayout.addView(edit);
        Button raw = new Button(this);
        raw.setText("查看原始详情");
        contentLayout.addView(raw);
        TextView content = new TextView(this);
        content.setTypeface(Typeface.MONOSPACE);
        content.setTextSize(12);
        content.setTextIsSelectable(true);
        content.setPadding(20, 12, 20, 12);
        content.setText(formatFlowDetail(flow));
        ScrollView scroll = new ScrollView(this);
        contentLayout.addView(content);
        scroll.addView(contentLayout);
        AlertDialog.Builder dialog = new AlertDialog.Builder(this)
            .setTitle("请求详情")
            .setView(scroll)
            .setNegativeButton("关闭", null);
        JSONObject bodies = flow.optJSONObject("bodyReferences");
        if (bodies != null && bodies.has("request")) {
            dialog.setPositiveButton("请求正文", (ignored, which) -> showBodyPreview(flow, "request"));
        }
        if (bodies != null && bodies.has("response")) {
            dialog.setNeutralButton("响应正文", (ignored, which) -> showBodyPreview(flow, "response"));
        }
        AlertDialog shown = dialog.show();
        edit.setOnClickListener(view -> {
            shown.dismiss();
            editRecordedRequest(flow);
        });
        raw.setOnClickListener(view -> {
            if ("查看原始详情".contentEquals(raw.getText())) {
                try { content.setText(flow.toString(2)); }
                catch (JSONException error) { content.setText(flow.toString()); }
                raw.setText("查看摘要与请求头");
            } else {
                content.setText(formatFlowDetail(flow));
                raw.setText("查看原始详情");
            }
        });
    }

    private static String formatFlowDetail(JSONObject flow) {
        StringBuilder text = new StringBuilder();
        JSONObject request = flow.optJSONObject("request");
        JSONObject response = flow.optJSONObject("response");
        text.append(request == null ? "请求" : request.optString("method", "请求"))
            .append(' ').append(request == null ? "" : request.optString("url"))
            .append("\n\n状态：").append(flow.optString("state", "未知"));
        if (response != null) text.append(" · HTTP ").append(response.optInt("status"));
        text.append("\n耗时：").append(flow.optLong("durationMs")).append(" ms");
        String protocol = flow.optString("clientProtocol");
        if (!protocol.isEmpty()) text.append("\n客户端协议：").append(protocol);
        JSONObject origin = flow.optJSONObject("trafficOrigin");
        if (origin != null) {
            text.append("\n来源：").append(origin.optString("entry", "未知"));
            JSONObject process = origin.optJSONObject("process");
            if (process != null) text.append(" · ").append(process.optString("name"));
        }
        String error = flow.optString("error");
        if (!error.isEmpty() && !"null".equals(error)) text.append("\n错误：").append(error);
        appendFlowHeaders(text, "请求头", request);
        appendFlowHeaders(text, "响应头", response);
        JSONObject bodies = flow.optJSONObject("bodyReferences");
        if (bodies != null) {
            text.append("\n\n正文大小");
            for (String kind : new String[] {"request", "response"}) {
                JSONObject body = bodies.optJSONObject(kind);
                if (body != null) text.append("\n")
                    .append("request".equals(kind) ? "请求" : "响应")
                    .append("：").append(body.optLong("byteLength")).append(" 字节");
            }
        }
        JSONArray notes = flow.optJSONArray("notes");
        if (notes != null && notes.length() > 0) {
            text.append("\n\n说明");
            for (int i = 0; i < notes.length(); i++) text.append("\n").append(notes.optString(i));
        }
        return text.toString();
    }

    private static void appendFlowHeaders(StringBuilder text, String title, JSONObject side) {
        if (side == null) return;
        JSONArray headers = side.optJSONArray("headers");
        text.append("\n\n").append(title);
        if (headers == null || headers.length() == 0) {
            text.append("\n（无）");
            return;
        }
        for (int i = 0; i < headers.length(); i++) {
            JSONObject header = headers.optJSONObject(i);
            if (header == null) continue;
            text.append("\n");
            if (header.optBoolean("disabled")) text.append("[停用] ");
            text.append(header.optString("name")).append(": ").append(header.optString("value"));
        }
    }

    private void showBodyPreview(JSONObject flow, String variant) {
        showBodyPreview(flow, variant, 0, new ArrayList<>());
    }

    private void showBodyPreview(JSONObject flow, String variant, long offset,
                                 ArrayList<Long> previousOffsets) {
        String id = flow.optString("id");
        String bodyRevision = flow.optString("bodyRevision");
        worker.execute(() -> {
            JSONObject reply = parseReply(NativeCapture.bodyPreview(id, variant, bodyRevision, offset));
            main.post(() -> {
                if (!reply.optBoolean("ok")) {
                    status.setText("读取正文失败：" + reply.optString("error"));
                    return;
                }
                JSONObject chunk = reply.optJSONObject("value");
                if (chunk == null) return;
                String text = chunk.isNull("text")
                    ? "二进制内容（Base64）：\n" + chunk.optString("bodyBase64")
                    : chunk.optString("text");
                long totalBytes = chunk.optLong("totalBytes");
                long currentOffset = chunk.optLong("offset", offset);
                long nextOffset = chunk.isNull("nextOffset") ? -1 : chunk.optLong("nextOffset");
                TextView content = new TextView(this);
                content.setTypeface(Typeface.MONOSPACE);
                content.setTextSize(12);
                content.setTextIsSelectable(true);
                content.setPadding(20, 12, 20, 12);
                content.setText(text);
                ScrollView scroll = new ScrollView(this);
                scroll.addView(content);
                String label = "request".equals(variant) ? "请求正文" : "响应正文";
                String position = totalBytes == 0 ? "0 / 0"
                    : (currentOffset + 1) + "–" + Math.min(totalBytes, nextOffset >= 0 ? nextOffset : totalBytes)
                        + " / " + totalBytes;
                AlertDialog.Builder dialog = new AlertDialog.Builder(this)
                    .setTitle(label + " · " + position + " 字节")
                    .setView(scroll)
                    .setNeutralButton("关闭", null);
                if (!previousOffsets.isEmpty()) {
                    ArrayList<Long> earlierOffsets = new ArrayList<>(previousOffsets);
                    long previousOffset = earlierOffsets.remove(earlierOffsets.size() - 1);
                    dialog.setNegativeButton("上一段", (ignored, which) ->
                        showBodyPreview(flow, variant, previousOffset, earlierOffsets));
                }
                if (nextOffset >= 0) {
                    ArrayList<Long> laterOffsets = new ArrayList<>(previousOffsets);
                    laterOffsets.add(currentOffset);
                    dialog.setPositiveButton("下一段", (ignored, which) ->
                        showBodyPreview(flow, variant, nextOffset, laterOffsets));
                }
                dialog.show();
            });
        });
    }

    private void editRecordedRequest(JSONObject flow) {
        JSONObject request = flow.optJSONObject("request");
        JSONObject references = flow.optJSONObject("bodyReferences");
        JSONObject bodyReference = references == null ? null : references.optJSONObject("request");
        if (request == null || bodyReference == null) {
            showReplayError("这条记录缺少请求信息，请重新打开详情");
            return;
        }
        if (!java.util.Arrays.asList(SEND_ENGINES).contains(request.optString("engine", "auto"))) {
            showReplayError("原请求的发送引擎未内置于移动版，无法无损复制；可手动新建请求");
            return;
        }
        long bodyLength = bodyReference.optLong("byteLength", -1);
        if (bodyLength < 0 || bodyLength > 64 * 1024) {
            showReplayError("请求正文超过 64 KiB，当前移动编辑器尚不能安全编辑后重发");
            return;
        }
        StringBuilder headerText = new StringBuilder();
        JSONArray headerRows = request.optJSONArray("headers");
        if (headerRows != null) {
            for (int i = 0; i < headerRows.length(); i++) {
                JSONObject row = headerRows.optJSONObject(i);
                if (row == null || row.optBoolean("disabled")) continue;
                String name = row.optString("name");
                if (name.startsWith(":")) continue;
                headerText.append(name).append(": ").append(row.optString("value")).append('\n');
            }
        }
        if (headerText.toString().getBytes(StandardCharsets.UTF_8).length > 16 * 1024) {
            showReplayError("请求头超过 16 KiB，当前移动编辑器尚不能安全编辑后重发");
            return;
        }
        worker.execute(() -> {
            try {
                ByteArrayOutputStream bytes = new ByteArrayOutputStream((int) bodyLength);
                long offset = 0;
                while (offset < bodyLength) {
                    JSONObject reply = parseReply(NativeCapture.bodyPreview(
                        flow.optString("id"), "request", flow.optString("bodyRevision"), offset));
                    if (!reply.optBoolean("ok")) throw new IllegalStateException(reply.optString("error"));
                    JSONObject chunk = reply.optJSONObject("value");
                    if (chunk == null) throw new IllegalStateException("读取请求正文失败");
                    byte[] part = Base64.decode(chunk.optString("bodyBase64"), Base64.DEFAULT);
                    if (part.length == 0 || bytes.size() + part.length > bodyLength) {
                        throw new IllegalStateException("请求正文分段不完整");
                    }
                    bytes.write(part, 0, part.length);
                    offset += part.length;
                }
                String bodyText = StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(bytes.toByteArray())).toString();
                main.post(() -> openManualRequest(request.optString("method", "GET"),
                    request.optString("url"), headerText.toString(), bodyText,
                    request.optString("engine", "auto")));
            } catch (Exception error) {
                main.post(() -> showReplayError(error.getMessage()));
            }
        });
    }

    private void showReplayError(String reason) {
        new AlertDialog.Builder(this).setTitle("无法复制为新请求")
            .setMessage(reason).setPositiveButton("关闭", null).show();
    }

    private void openManualRequest() {
        openManualRequest(new ManualRequestDraft());
    }

    private void openManualRequest(String initialMethod, String initialUrl,
                                   String initialHeaders, String initialBody, String initialEngine) {
        ManualRequestDraft draft = new ManualRequestDraft();
        draft.method = initialMethod;
        draft.url = initialUrl;
        draft.headers = initialHeaders;
        draft.body = initialBody;
        draft.engine = initialEngine;
        openManualRequest(draft);
    }

    private void openManualRequest(ManualRequestDraft draft) {
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        int inset = (int) (12 * getResources().getDisplayMetrics().density);
        content.setPadding(inset, inset, inset, inset);
        EditText method = new EditText(this);
        method.setHint("方法，例如 GET、POST");
        method.setSingleLine(true);
        method.setText(draft.method);
        content.addView(method);
        EditText url = new EditText(this);
        url.setHint("https://example.com/");
        url.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        url.setSingleLine(true);
        url.setText(draft.url);
        content.addView(url);
        TextView engineLabel = new TextView(this);
        engineLabel.setText("发送协议（移动端使用原生 TLS）");
        content.addView(engineLabel);
        Spinner engine = new Spinner(this);
        engine.setAdapter(new ArrayAdapter<>(this,
            android.R.layout.simple_spinner_dropdown_item, SEND_ENGINE_LABELS));
        int engineIndex = java.util.Arrays.asList(SEND_ENGINES).indexOf(draft.engine);
        engine.setSelection(Math.max(0, engineIndex));
        content.addView(engine);
        EditText headers = new EditText(this);
        headers.setHint("请求头，每行一个：\nContent-Type: application/json");
        headers.setMinLines(3);
        headers.setGravity(Gravity.TOP);
        headers.setText(draft.headers);
        content.addView(headers);
        TextView modeLabel = new TextView(this);
        modeLabel.setText("请求正文类型");
        content.addView(modeLabel);
        Spinner bodyMode = new Spinner(this);
        bodyMode.setAdapter(new ArrayAdapter<>(this,
            android.R.layout.simple_spinner_dropdown_item, BODY_MODES));
        bodyMode.setSelection(Math.max(0, Math.min(BODY_MODES.length - 1, draft.bodyMode)));
        content.addView(bodyMode);
        EditText body = new EditText(this);
        body.setHint("文本 / JSON：直接输入；表单：每行 名称=值；Base64：粘贴编码内容");
        body.setMinLines(4);
        body.setGravity(Gravity.TOP);
        body.setText(draft.body);
        content.addView(body);
        TextView fileLabel = new TextView(this);
        fileLabel.setText(draft.fileUri == null ? "未选择文件" : "文件："
            + (draft.fileName == null ? "attachment.bin" : draft.fileName));
        content.addView(fileLabel);
        Button pickFile = new Button(this);
        pickFile.setText("选择文件（文件 / Multipart）");
        content.addView(pickFile);
        ScrollView scroll = new ScrollView(this);
        scroll.addView(content);
        AlertDialog dialog = new AlertDialog.Builder(this)
            .setTitle("新建 HTTP 请求")
            .setView(scroll)
            .setNegativeButton("取消", null)
            .setPositiveButton("发送", null)
            .create();
        pickFile.setOnClickListener(view -> {
            draft.method = method.getText().toString();
            draft.url = url.getText().toString();
            draft.headers = headers.getText().toString();
            draft.body = body.getText().toString();
            draft.engine = SEND_ENGINES[engine.getSelectedItemPosition()];
            draft.bodyMode = bodyMode.getSelectedItemPosition();
            pendingFileRequest = draft;
            Intent pick = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            pick.addCategory(Intent.CATEGORY_OPENABLE);
            pick.setType("*/*");
            dialog.dismiss();
            startActivityForResult(pick, PICK_REQUEST_FILE);
        });
        bodyMode.setOnItemSelectedListener(new android.widget.AdapterView.OnItemSelectedListener() {
            @Override public void onItemSelected(android.widget.AdapterView<?> parent, View view,
                                                 int position, long id) {
                body.setVisibility(position == 3 ? View.GONE : View.VISIBLE);
                int fileVisibility = position == 3 || position == 4 ? View.VISIBLE : View.GONE;
                pickFile.setVisibility(fileVisibility);
                fileLabel.setVisibility(fileVisibility);
            }
            @Override public void onNothingSelected(android.widget.AdapterView<?> parent) { }
        });
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            draft.method = method.getText().toString();
            draft.url = url.getText().toString();
            draft.headers = headers.getText().toString();
            draft.body = body.getText().toString();
            draft.engine = SEND_ENGINES[engine.getSelectedItemPosition()];
            draft.bodyMode = bodyMode.getSelectedItemPosition();
            if (sendManualRequest(draft)) {
                dialog.dismiss();
            }
        }));
        dialog.show();
    }

    private boolean sendManualRequest(ManualRequestDraft draft) {
        String requestMethod = draft.method.trim().toUpperCase(Locale.ROOT);
        String requestUrl = draft.url.trim();
        try {
            URI uri = new URI(requestUrl);
            if (requestMethod.isEmpty() || uri.getHost() == null
                    || !("http".equalsIgnoreCase(uri.getScheme())
                         || "https".equalsIgnoreCase(uri.getScheme()))) {
                throw new IllegalArgumentException("请输入 HTTP(S) URL 和请求方法");
            }
            if (!java.util.Arrays.asList(SEND_ENGINES).contains(draft.engine)) {
                throw new IllegalArgumentException("此发送引擎未内置于移动版，请选择原生协议");
            }
            if ("h3".equals(draft.engine) && !"https".equalsIgnoreCase(uri.getScheme())) {
                throw new IllegalArgumentException("HTTP/3 需要 HTTPS URL");
            }
            if (draft.body.getBytes(StandardCharsets.UTF_8).length > 64 * 1024
                    || draft.headers.getBytes(StandardCharsets.UTF_8).length > 16 * 1024) {
                throw new IllegalArgumentException("文本输入最多 64 KiB，请求头最多 16 KiB");
            }
            if (draft.bodyMode == 3 && draft.fileUri == null) {
                throw new IllegalArgumentException("请先选择要发送的文件");
            }
            JSONArray headerRows = new JSONArray();
            for (String line : draft.headers.split("\\r?\\n")) {
                if (line.trim().isEmpty()) continue;
                int separator = line.indexOf(':');
                if (separator <= 0) throw new IllegalArgumentException("请求头需要按 名称: 值 填写");
                headerRows.put(new JSONObject()
                    .put("name", line.substring(0, separator).trim())
                    .put("value", line.substring(separator + 1).trim()));
            }
            status.setText("正在发送请求…");
            worker.execute(() -> {
                try {
                    RequestBody requestBody = composeRequestBody(draft);
                    JSONArray preparedHeaders = applyContentType(headerRows,
                        requestBody.contentType, requestBody.replaceContentType);
                    JSONObject request = new JSONObject()
                        .put("method", requestMethod)
                        .put("url", requestUrl)
                        .put("engine", draft.engine)
                        .put("headers", preparedHeaders)
                        .put("bodyBase64", Base64.encodeToString(requestBody.bytes, Base64.NO_WRAP));
                    JSONObject reply = parseReply(NativeCapture.sendRequest(request.toString()));
                    main.post(() -> {
                        if (!reply.optBoolean("ok")) {
                            status.setText("发送失败：" + reply.optString("error"));
                            if (!isFinishing() && !isDestroyed()) openManualRequest(draft);
                            return;
                        }
                        JSONObject value = reply.optJSONObject("value");
                        if (value == null) return;
                        status.setText(value.isNull("error")
                            ? "已发送，响应状态 " + value.optString("status", "未知")
                            : "请求失败：" + value.optString("error"));
                        poll();
                        showFlowDetail(value.optString("id"));
                    });
                } catch (Exception error) {
                    main.post(() -> {
                        status.setText("发送失败：" + error.getMessage());
                        if (!isFinishing() && !isDestroyed()) openManualRequest(draft);
                    });
                }
            });
            return true;
        } catch (Exception error) {
            new AlertDialog.Builder(this).setTitle("请求输入有误")
                .setMessage(error.getMessage())
                .setPositiveButton("关闭", null).show();
            return false;
        }
    }

    private static final class RequestBody {
        final byte[] bytes;
        final String contentType;
        final boolean replaceContentType;
        RequestBody(byte[] bytes, String contentType, boolean replaceContentType) {
            this.bytes = bytes;
            this.contentType = contentType;
            this.replaceContentType = replaceContentType;
        }
    }

    private RequestBody composeRequestBody(ManualRequestDraft draft) throws Exception {
        byte[] bytes;
        String contentType = null;
        boolean replaceContentType = false;
        switch (draft.bodyMode) {
            case 1: // JSON
                if (!draft.body.trim().isEmpty()) {
                    JSONTokener parser = new JSONTokener(draft.body);
                    parser.nextValue();
                    if (parser.nextClean() != 0) throw new IllegalArgumentException("JSON 后还有多余内容");
                }
                bytes = draft.body.getBytes(StandardCharsets.UTF_8);
                contentType = "application/json; charset=utf-8";
                break;
            case 2: // application/x-www-form-urlencoded
                bytes = RequestBodyCodec.urlForm(draft.body);
                contentType = "application/x-www-form-urlencoded";
                break;
            case 3: // Raw file
                if (draft.fileUri == null) throw new IllegalArgumentException("请先选择要发送的文件");
                bytes = readRequestFile(draft.fileUri);
                contentType = RequestBodyCodec.safeMimeType(getContentResolver().getType(draft.fileUri));
                break;
            case 4: // Multipart form fields and optional file
                String boundary = "----HttpCapture" + UUID.randomUUID().toString().replace("-", "");
                bytes = RequestBodyCodec.multipart(draft.body, boundary, draft.fileName,
                    draft.fileUri == null ? null : getContentResolver().getType(draft.fileUri),
                    draft.fileUri == null ? null : readRequestFile(draft.fileUri));
                contentType = "multipart/form-data; boundary=" + boundary;
                replaceContentType = true;
                break;
            case 5: // Raw bytes pasted as Base64
                bytes = Base64.decode(draft.body.replaceAll("\\s", ""), Base64.DEFAULT);
                break;
            case 0:
                bytes = draft.body.getBytes(StandardCharsets.UTF_8);
                break;
            default:
                throw new IllegalArgumentException("未知的正文类型");
        }
        if (bytes.length > RequestBodyCodec.MAX_BODY_BYTES) {
            throw new IllegalArgumentException("请求正文最多 1 MiB");
        }
        return new RequestBody(bytes, contentType, replaceContentType);
    }

    private byte[] readRequestFile(Uri uri) throws Exception {
        try (InputStream input = getContentResolver().openInputStream(uri);
             ByteArrayOutputStream output = new ByteArrayOutputStream()) {
            if (input == null) throw new IllegalArgumentException("无法打开所选文件");
            byte[] buffer = new byte[8192];
            int count;
            while ((count = input.read(buffer)) != -1) {
                if (output.size() + count > RequestBodyCodec.MAX_BODY_BYTES) {
                    throw new IllegalArgumentException("文件超过 1 MiB，移动编辑器暂不支持发送");
                }
                output.write(buffer, 0, count);
            }
            return output.toByteArray();
        }
    }

    private static JSONArray applyContentType(JSONArray headers, String contentType,
                                              boolean replace) throws JSONException {
        if (contentType == null) return headers;
        JSONArray result = new JSONArray();
        boolean found = false;
        for (int i = 0; i < headers.length(); i++) {
            JSONObject header = headers.getJSONObject(i);
            if ("Content-Type".equalsIgnoreCase(header.optString("name"))) {
                if (replace) continue;
                found = true;
            }
            result.put(header);
        }
        if (!found) result.put(new JSONObject().put("name", "Content-Type").put("value", contentType));
        return result;
    }

    private String requestFileName(Uri uri) {
        try (Cursor cursor = getContentResolver().query(uri,
                new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                String name = cursor.getString(0);
                if (name != null && !name.trim().isEmpty()) return name;
            }
        } catch (RuntimeException ignored) { }
        return "attachment.bin";
    }

    private void loadBlocking() {
        worker.execute(() -> {
            JSONObject reply = parseReply(NativeCapture.blocking());
            main.post(() -> {
                if (reply.optBoolean("ok")) {
                    JSONObject config = reply.optJSONObject("value");
                    if (config != null) showBlocking(config);
                } else {
                    status.setText("读取阻止规则失败：" + reply.optString("error"));
                }
            });
        });
    }

    private void saveBlocking(JSONObject config) {
        worker.execute(() -> {
            JSONObject reply = parseReply(NativeCapture.configureBlocking(config.toString()));
            main.post(() -> {
                if (reply.optBoolean("ok")) {
                    JSONObject saved = reply.optJSONObject("value");
                    if (saved != null) showBlocking(saved);
                } else {
                    new AlertDialog.Builder(this).setTitle("规则保存失败")
                        .setMessage(reply.optString("error"))
                        .setPositiveButton("关闭", null).show();
                }
            });
        });
    }

    private void showBlocking(JSONObject config) {
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        int inset = (int) (12 * getResources().getDisplayMetrics().density);
        content.setPadding(inset, inset, inset, inset);
        CheckBox enabled = new CheckBox(this);
        enabled.setText("启用 HTTP 请求阻止");
        enabled.setChecked(config.optBoolean("enabled"));
        content.addView(enabled);
        TextView scopeLabel = new TextView(this);
        scopeLabel.setText("作用范围");
        content.addView(scopeLabel);
        Spinner scope = new Spinner(this);
        String[] scopeValues = {"capture", "replay", "all"};
        ArrayAdapter<String> scopes = new ArrayAdapter<>(this,
            android.R.layout.simple_spinner_item, new String[] {"仅抓包", "仅重放", "抓包和重放"});
        scopes.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        scope.setAdapter(scopes);
        int selectedScope = java.util.Arrays.asList(scopeValues).indexOf(config.optString("scope", "capture"));
        scope.setSelection(Math.max(0, selectedScope));
        content.addView(scope);
        Button saveScope = new Button(this);
        saveScope.setText("保存作用范围");
        content.addView(saveScope);
        JSONArray rules = config.optJSONArray("rules");
        if (rules == null) {
            rules = new JSONArray();
            try { config.put("rules", rules); }
            catch (JSONException ignored) { return; }
        }
        final JSONArray ruleList = rules;
        List<LinearLayout> ruleLines = new ArrayList<>();
        List<JSONObject> ruleObjects = new ArrayList<>();
        List<Integer> ruleIndices = new ArrayList<>();
        for (int i = 0; i < ruleList.length(); i++) {
            JSONObject rule = ruleList.optJSONObject(i);
            if (rule == null) continue;
            LinearLayout line = new LinearLayout(this);
            line.setOrientation(LinearLayout.VERTICAL);
            CheckBox ruleEnabled = new CheckBox(this);
            String ruleMethod = rule.optString("method");
            ruleEnabled.setText(rule.optString("pattern") + " · " + rule.optString("matchType")
                + " · " + (ruleMethod.isEmpty() ? "全部方法" : ruleMethod));
            ruleEnabled.setChecked(rule.optBoolean("enabled"));
            line.addView(ruleEnabled);
            LinearLayout actions = new LinearLayout(this);
            actions.setOrientation(LinearLayout.HORIZONTAL);
            Button edit = new Button(this);
            edit.setText("编辑");
            actions.addView(edit);
            Button delete = new Button(this);
            delete.setText("删除");
            actions.addView(delete);
            line.addView(actions);
            content.addView(line);
            ruleLines.add(line);
            ruleObjects.add(rule);
            ruleIndices.add(i);
        }
        Spinner matchType = new Spinner(this);
        String[] matchLabels = {"精确域名", "URL 包含", "URL 正则"};
        String[] matchValues = {"host", "urlContains", "urlRegex"};
        ArrayAdapter<String> matches = new ArrayAdapter<>(this,
            android.R.layout.simple_spinner_item, matchLabels);
        matches.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        matchType.setAdapter(matches);
        content.addView(matchType);
        EditText patternInput = new EditText(this);
        patternInput.setSingleLine(true);
        patternInput.setHint("域名或 URL 匹配内容");
        patternInput.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        content.addView(patternInput);
        EditText methodInput = new EditText(this);
        methodInput.setSingleLine(true);
        methodInput.setHint("方法（留空表示全部）");
        content.addView(methodInput);
        Button add = new Button(this);
        add.setText("添加阻止规则");
        content.addView(add);
        ScrollView scroll = new ScrollView(this);
        scroll.addView(content);
        AlertDialog dialog = new AlertDialog.Builder(this)
            .setTitle("阻止规则")
            .setView(scroll)
            .setNegativeButton("关闭", null)
            .create();
        enabled.setOnCheckedChangeListener((button, checked) -> {
            try { config.put("enabled", checked); }
            catch (JSONException ignored) { return; }
            dialog.dismiss();
            saveBlocking(config);
        });
        saveScope.setOnClickListener(view -> {
            String selected = scopeValues[scope.getSelectedItemPosition()];
            if (selected.equals(config.optString("scope", "capture"))) return;
            try { config.put("scope", selected); }
            catch (JSONException ignored) { return; }
            dialog.dismiss();
            saveBlocking(config);
        });
        for (int i = 0; i < ruleLines.size(); i++) {
            JSONObject rule = ruleObjects.get(i);
            LinearLayout line = ruleLines.get(i);
            CheckBox ruleEnabled = (CheckBox) line.getChildAt(0);
            LinearLayout actions = (LinearLayout) line.getChildAt(1);
            Button edit = (Button) actions.getChildAt(0);
            Button delete = (Button) actions.getChildAt(1);
            final int index = ruleIndices.get(i);
            ruleEnabled.setOnCheckedChangeListener((button, checked) -> {
                try { rule.put("enabled", checked); }
                catch (JSONException ignored) { return; }
                dialog.dismiss();
                saveBlocking(config);
            });
            edit.setOnClickListener(view -> {
                dialog.dismiss();
                editBlockingRule(config, rule);
            });
            delete.setOnClickListener(view -> {
                ruleList.remove(index);
                dialog.dismiss();
                saveBlocking(config);
            });
        }
        add.setOnClickListener(view -> {
            String pattern = patternInput.getText().toString().trim();
            if (pattern.isEmpty()) {
                patternInput.setError("请填写匹配内容");
                return;
            }
            try {
                JSONObject rule = new JSONObject()
                    .put("id", "mobile-" + UUID.randomUUID())
                    .put("enabled", true)
                    .put("matchType", matchValues[matchType.getSelectedItemPosition()])
                    .put("pattern", pattern)
                    .put("method", methodInput.getText().toString().trim().toUpperCase(Locale.ROOT));
                ruleList.put(rule);
                config.put("enabled", true);
            } catch (JSONException ignored) { return; }
            dialog.dismiss();
            saveBlocking(config);
        });
        dialog.show();
    }

    private void editBlockingRule(JSONObject config, JSONObject rule) {
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        int inset = (int) (12 * getResources().getDisplayMetrics().density);
        content.setPadding(inset, inset, inset, inset);
        Spinner matchType = new Spinner(this);
        String[] matchValues = {"host", "urlContains", "urlRegex"};
        ArrayAdapter<String> matches = new ArrayAdapter<>(this,
            android.R.layout.simple_spinner_item, new String[] {"精确域名", "URL 包含", "URL 正则"});
        matches.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        matchType.setAdapter(matches);
        matchType.setSelection(Math.max(0,
            java.util.Arrays.asList(matchValues).indexOf(rule.optString("matchType"))));
        content.addView(matchType);
        EditText pattern = new EditText(this);
        pattern.setSingleLine(true);
        pattern.setHint("域名或 URL 匹配内容");
        pattern.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        pattern.setText(rule.optString("pattern"));
        content.addView(pattern);
        EditText method = new EditText(this);
        method.setSingleLine(true);
        method.setHint("方法（留空表示全部）");
        method.setText(rule.optString("method"));
        content.addView(method);
        AlertDialog editor = new AlertDialog.Builder(this)
            .setTitle("编辑阻止规则")
            .setView(content)
            .setNegativeButton("取消", (ignored, which) -> showBlocking(config))
            .setPositiveButton("保存修改", null)
            .create();
        editor.setOnShowListener(ignored -> editor.getButton(AlertDialog.BUTTON_POSITIVE)
            .setOnClickListener(view -> {
                String value = pattern.getText().toString().trim();
                if (value.isEmpty()) {
                    pattern.setError("请填写匹配内容");
                    return;
                }
                try {
                    rule.put("matchType", matchValues[matchType.getSelectedItemPosition()]);
                    rule.put("pattern", value);
                    rule.put("method", method.getText().toString().trim().toUpperCase(Locale.ROOT));
                } catch (JSONException ignoredError) { return; }
                editor.dismiss();
                saveBlocking(config);
            }));
        editor.show();
    }

    private void loadResponseRewrite() {
        worker.execute(() -> {
            JSONObject reply = parseReply(NativeCapture.responseRewrite());
            main.post(() -> {
                JSONObject snapshot = reply.optJSONObject("value");
                if (reply.optBoolean("ok") && snapshot != null) showResponseRewrite(snapshot);
                else status.setText("读取响应修改规则失败：" + reply.optString("error"));
            });
        });
    }

    private void saveResponseRewrite(JSONObject snapshot) {
        saveResponseRewrite(snapshot, null);
    }

    private void saveResponseRewrite(JSONObject snapshot, AlertDialog editor) {
        worker.execute(() -> {
            try {
                JSONObject update = new JSONObject()
                    .put("config", snapshot.getJSONObject("config"))
                    .put("expectedVersion", snapshot.getString("version"));
                JSONObject reply = parseReply(NativeCapture.configureResponseRewrite(update.toString()));
                main.post(() -> {
                    JSONObject saved = reply.optJSONObject("value");
                    if (reply.optBoolean("ok") && saved != null) {
                        if (editor != null) editor.dismiss();
                        showResponseRewrite(saved);
                    } else {
                        if (editor != null) editor.getButton(AlertDialog.BUTTON_POSITIVE).setEnabled(true);
                        new AlertDialog.Builder(this).setTitle("响应修改规则未保存")
                        .setMessage(reply.optString("error"))
                        .setPositiveButton(editor == null ? "重新加载" : "继续编辑",
                            (ignored, which) -> { if (editor == null) loadResponseRewrite(); })
                        .show();
                    }
                });
            } catch (JSONException error) {
                main.post(() -> status.setText("响应修改配置无效：" + error.getMessage()));
            }
        });
    }

    private void showResponseRewrite(JSONObject snapshot) {
        JSONObject config = snapshot.optJSONObject("config");
        if (config == null) return;
        JSONArray rules = config.optJSONArray("rules");
        if (rules == null) return;
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        int inset = (int) (12 * getResources().getDisplayMetrics().density);
        content.setPadding(inset, inset, inset, inset);
        CheckBox enabled = new CheckBox(this);
        enabled.setText("启用响应修改");
        enabled.setChecked(config.optBoolean("enabled"));
        content.addView(enabled);
        Button scope = new Button(this);
        String currentScope = config.optString("scope", "capture");
        scope.setText("作用范围：" + ("all".equals(currentScope) ? "抓包和重放"
            : "replay".equals(currentScope) ? "仅重放" : "仅抓包"));
        content.addView(scope);
        JSONArray hits = snapshot.optJSONArray("hits");
        final AlertDialog[] dialogHolder = new AlertDialog[1];
        for (int i = 0; i < rules.length(); i++) {
            JSONObject rule = rules.optJSONObject(i);
            if (rule == null) continue;
            int index = i;
            LinearLayout row = new LinearLayout(this);
            row.setOrientation(LinearLayout.VERTICAL);
            CheckBox active = new CheckBox(this);
            String name = rule.optString("name");
            if (name.isEmpty()) name = rule.optString("pattern");
            active.setText(name + " · " + ("modify".equals(rule.optString("action")) ? "局部修改" : "整份替换")
                + " · 命中 " + (hits == null ? 0 : hits.optLong(i)));
            active.setChecked(rule.optBoolean("enabled"));
            row.addView(active);
            LinearLayout actions = new LinearLayout(this);
            actions.setOrientation(LinearLayout.HORIZONTAL);
            Button edit = new Button(this);
            edit.setText("编辑");
            edit.setEnabled(canEditResponseRewriteRule(rule));
            actions.addView(edit);
            Button delete = new Button(this);
            delete.setText("删除");
            actions.addView(delete);
            row.addView(actions);
            content.addView(row);
            active.setOnCheckedChangeListener((button, checked) -> {
                try { rule.put("enabled", checked); }
                catch (JSONException ignored) { return; }
                dialogHolder[0].dismiss();
                saveResponseRewrite(snapshot);
            });
            edit.setOnClickListener(view -> {
                dialogHolder[0].dismiss();
                addResponseRewriteRule(snapshot, "modify".equals(rule.optString("action")), index);
            });
            delete.setOnClickListener(view -> {
                rules.remove(index);
                dialogHolder[0].dismiss();
                saveResponseRewrite(snapshot);
            });
        }
        Button replace = new Button(this);
        replace.setText("添加整份响应替换");
        content.addView(replace);
        Button modify = new Button(this);
        modify.setText("添加正文局部替换");
        content.addView(modify);
        ScrollView scroll = new ScrollView(this);
        scroll.addView(content);
        AlertDialog dialog = new AlertDialog.Builder(this)
            .setTitle("响应修改规则")
            .setView(scroll)
            .setNegativeButton("关闭", null)
            .create();
        dialogHolder[0] = dialog;
        enabled.setOnCheckedChangeListener((button, checked) -> {
            try { config.put("enabled", checked); }
            catch (JSONException ignored) { return; }
            dialog.dismiss();
            saveResponseRewrite(snapshot);
        });
        scope.setOnClickListener(view -> {
            String next = "capture".equals(currentScope) ? "all"
                : "all".equals(currentScope) ? "replay" : "capture";
            try { config.put("scope", next); }
            catch (JSONException ignored) { return; }
            dialog.dismiss();
            saveResponseRewrite(snapshot);
        });
        replace.setOnClickListener(view -> {
            dialog.dismiss();
            addResponseRewriteRule(snapshot, false, -1);
        });
        modify.setOnClickListener(view -> {
            dialog.dismiss();
            addResponseRewriteRule(snapshot, true, -1);
        });
        dialog.show();
    }

    private boolean canEditResponseRewriteRule(JSONObject rule) {
        if (!"host".equals(rule.optString("matchType"))
                && !"urlContains".equals(rule.optString("matchType"))
                && !"urlRegex".equals(rule.optString("matchType"))) return false;
        if ("modify".equals(rule.optString("action"))) {
            JSONArray changes = rule.optJSONArray("modifications");
            if (rule.optJSONObject("replacement") != null || changes == null || changes.length() != 1) return false;
            JSONObject change = changes.optJSONObject(0);
            return change != null && "body".equals(change.optString("target"))
                && "replace".equals(change.optString("operation"))
                && change.optString("find").getBytes(StandardCharsets.UTF_8).length <= 64 * 1024
                && change.optString("value").getBytes(StandardCharsets.UTF_8).length <= 64 * 1024;
        }
        if (!"replace".equals(rule.optString("action"))) return false;
        JSONArray changes = rule.optJSONArray("modifications");
        if (changes != null && changes.length() > 0) return false;
        JSONObject replacement = rule.optJSONObject("replacement");
        if (replacement == null) return false;
        try {
            String bodyBase64 = replacement.getString("bodyBase64");
            if (bodyBase64.length() > 96 * 1024) return false;
            byte[] bytes = Base64.decode(bodyBase64, Base64.DEFAULT);
            if (bytes.length > 64 * 1024
                    || !java.util.Arrays.equals(bytes, new String(bytes, StandardCharsets.UTF_8)
                        .getBytes(StandardCharsets.UTF_8))) return false;
            JSONArray headers = replacement.getJSONArray("headers");
            int length = 0;
            for (int i = 0; i < headers.length(); i++) {
                JSONObject header = headers.getJSONObject(i);
                String headerValue = header.getString("value");
                if (!headerValue.equals(headerValue.trim())) return false;
                length += header.getString("name").length() + headerValue.length() + 4;
            }
            return length <= 16 * 1024;
        } catch (Exception error) { return false; }
    }

    private void addResponseRewriteRule(JSONObject snapshot, boolean modify, int editIndex) {
        boolean editing = editIndex >= 0;
        JSONObject original = editing ? snapshot.optJSONObject("config").optJSONArray("rules").optJSONObject(editIndex) : null;
        if (editing && (original == null || !canEditResponseRewriteRule(original))) return;
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        int inset = (int) (12 * getResources().getDisplayMetrics().density);
        content.setPadding(inset, inset, inset, inset);
        EditText name = new EditText(this);
        name.setHint("规则名称（可选）");
        if (editing) name.setText(original.optString("name"));
        content.addView(name);
        Spinner match = new Spinner(this);
        String[] matchLabels = {"精确域名", "URL 包含", "URL 正则"};
        String[] matchValues = {"host", "urlContains", "urlRegex"};
        ArrayAdapter<String> choices = new ArrayAdapter<>(this,
            android.R.layout.simple_spinner_item, matchLabels);
        choices.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        match.setAdapter(choices);
        if (editing) {
            for (int i = 0; i < matchValues.length; i++) {
                if (matchValues[i].equals(original.optString("matchType"))) match.setSelection(i);
            }
        }
        content.addView(match);
        EditText pattern = new EditText(this);
        pattern.setHint("域名或 URL 匹配内容");
        pattern.setSingleLine(true);
        if (editing) pattern.setText(original.optString("pattern"));
        content.addView(pattern);
        EditText method = new EditText(this);
        method.setHint("请求方法（留空表示全部）");
        method.setSingleLine(true);
        if (editing) method.setText(original.optString("method"));
        content.addView(method);
        EditText statusCode = null;
        EditText headers = null;
        EditText body = null;
        EditText find = null;
        EditText value = null;
        CheckBox regex = null;
        if (modify) {
            find = new EditText(this);
            find.setHint("在响应正文中查找");
            if (editing) find.setText(original.optJSONArray("modifications").optJSONObject(0).optString("find"));
            content.addView(find);
            value = new EditText(this);
            value.setHint("替换为（可留空）");
            if (editing) value.setText(original.optJSONArray("modifications").optJSONObject(0).optString("value"));
            content.addView(value);
            regex = new CheckBox(this);
            regex.setText("按正则表达式查找");
            if (editing) regex.setChecked(original.optJSONArray("modifications").optJSONObject(0).optBoolean("regex"));
            content.addView(regex);
        } else {
            statusCode = new EditText(this);
            statusCode.setHint("响应状态码，例如 200");
            statusCode.setText("200");
            if (editing) statusCode.setText(String.valueOf(original.optJSONObject("replacement").optInt("status")));
            statusCode.setInputType(InputType.TYPE_CLASS_NUMBER);
            content.addView(statusCode);
            headers = new EditText(this);
            headers.setHint("响应头，每行一个，例如 Content-Type: application/json");
            headers.setMinLines(3);
            headers.setGravity(Gravity.TOP);
            if (editing) {
                JSONArray previousHeaders = original.optJSONObject("replacement").optJSONArray("headers");
                StringBuilder lines = new StringBuilder();
                for (int i = 0; i < previousHeaders.length(); i++) {
                    JSONObject header = previousHeaders.optJSONObject(i);
                    if (header != null) lines.append(header.optString("name")).append(": ")
                        .append(header.optString("value")).append('\n');
                }
                headers.setText(lines.toString());
            }
            content.addView(headers);
            body = new EditText(this);
            body.setHint("完整响应正文（UTF-8 文本）");
            body.setMinLines(4);
            body.setGravity(Gravity.TOP);
            if (editing) body.setText(new String(Base64.decode(
                original.optJSONObject("replacement").optString("bodyBase64"), Base64.DEFAULT), StandardCharsets.UTF_8));
            content.addView(body);
        }
        final EditText finalStatus = statusCode;
        final EditText finalHeaders = headers;
        final EditText finalBody = body;
        final EditText finalFind = find;
        final EditText finalValue = value;
        final CheckBox finalRegex = regex;
        ScrollView scroll = new ScrollView(this);
        scroll.addView(content);
        AlertDialog dialog = new AlertDialog.Builder(this)
            .setTitle((editing ? "编辑" : "添加") + (modify ? "正文局部替换" : "整份响应替换"))
            .setView(scroll)
            .setNegativeButton("取消", null)
            .setPositiveButton("保存规则", null)
            .create();
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            try {
                JSONObject candidate = new JSONObject(snapshot.toString());
                JSONObject config = candidate.getJSONObject("config");
                JSONArray rules = config.getJSONArray("rules");
                if (!editing && rules.length() >= 32) throw new IllegalArgumentException("最多支持 32 条规则");
                String targetPattern = pattern.getText().toString().trim();
                if (targetPattern.isEmpty()) throw new IllegalArgumentException("请填写匹配内容");
                JSONObject rule = editing ? new JSONObject(original.toString()) : new JSONObject()
                    .put("id", "mobile-" + UUID.randomUUID())
                    .put("enabled", true);
                rule.put("name", name.getText().toString().trim())
                    .put("matchType", matchValues[match.getSelectedItemPosition()])
                    .put("pattern", targetPattern)
                    .put("method", method.getText().toString().trim().toUpperCase(Locale.ROOT));
                if (modify) {
                    String search = finalFind.getText().toString();
                    String replacement = finalValue.getText().toString();
                    if (search.isEmpty()) throw new IllegalArgumentException("请填写查找内容");
                    if (search.getBytes(StandardCharsets.UTF_8).length > 64 * 1024
                            || replacement.getBytes(StandardCharsets.UTF_8).length > 64 * 1024) {
                        throw new IllegalArgumentException("查找和替换文本各最多 64 KiB");
                    }
                    rule.put("action", "modify");
                    JSONObject change = editing ? new JSONObject(original.optJSONArray("modifications")
                        .optJSONObject(0).toString()) : new JSONObject()
                        .put("target", "body").put("operation", "replace");
                    rule.put("modifications", new JSONArray().put(change
                        .put("find", search).put("value", replacement)
                        .put("regex", finalRegex.isChecked())));
                } else {
                    int responseStatus = Integer.parseInt(finalStatus.getText().toString().trim());
                    if (responseStatus < 200 || responseStatus > 599) {
                        throw new IllegalArgumentException("响应状态码必须为 200–599");
                    }
                    String responseBody = finalBody.getText().toString();
                    String headerInput = finalHeaders.getText().toString();
                    if (responseBody.getBytes(StandardCharsets.UTF_8).length > 64 * 1024
                            || headerInput.getBytes(StandardCharsets.UTF_8).length > 16 * 1024) {
                        throw new IllegalArgumentException("正文最多 64 KiB，响应头最多 16 KiB");
                    }
                    JSONArray responseHeaders = new JSONArray();
                    for (String line : headerInput.split("\\r?\\n")) {
                        if (line.trim().isEmpty()) continue;
                        int separator = line.indexOf(':');
                        if (separator <= 0) throw new IllegalArgumentException("响应头需要按 名称: 值 填写");
                        responseHeaders.put(new JSONObject()
                            .put("name", line.substring(0, separator).trim())
                            .put("value", line.substring(separator + 1).trim()));
                    }
                    rule.put("action", "replace");
                    rule.put("replacement", new JSONObject()
                        .put("status", responseStatus)
                        .put("headers", responseHeaders)
                        .put("bodyBase64", Base64.encodeToString(
                            responseBody.getBytes(StandardCharsets.UTF_8), Base64.NO_WRAP)));
                }
                if (editing) rules.put(editIndex, rule);
                else rules.put(rule);
                if (!editing) config.put("enabled", true);
                dialog.getButton(AlertDialog.BUTTON_POSITIVE).setEnabled(false);
                saveResponseRewrite(candidate, dialog);
            } catch (Exception error) {
                new AlertDialog.Builder(this).setTitle("规则输入有误")
                    .setMessage(error.getMessage())
                    .setPositiveButton("关闭", null).show();
            }
        }));
        dialog.show();
    }

    private void saveCertificate() {
        worker.execute(() -> {
            JSONObject reply = parseReply(NativeCapture.certificate());
            if (!reply.optBoolean("ok")) {
                main.post(() -> status.setText("读取 CA 失败：" + reply.optString("error")));
                return;
            }
            try {
                certificatePem = reply.getString("value");
                main.post(() -> {
                    Intent save = new Intent(Intent.ACTION_CREATE_DOCUMENT);
                    save.addCategory(Intent.CATEGORY_OPENABLE);
                    save.setType("application/x-x509-ca-cert");
                    save.putExtra(Intent.EXTRA_TITLE, "http-capture-ca.crt");
                    startActivityForResult(save, SAVE_CA_REQUEST);
                });
            } catch (Exception error) {
                main.post(() -> status.setText("导出 CA 失败：" + error.getMessage()));
            }
        });
    }

    @Override protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == VPN_REQUEST) {
            if (!vpnPermissionPending) return;
            vpnPermissionPending = false;
            if (resultCode == RESULT_OK) startVpnService(pendingVpnPort);
            else {
                pendingVpnPort = 0;
                status.setText("VPN 未授权；本机代理仍在运行，可继续导出 CA 证书");
            }
            return;
        }
        if (requestCode == PICK_REQUEST_FILE) {
            ManualRequestDraft draft = pendingFileRequest;
            pendingFileRequest = null;
            if (draft == null) return;
            if (resultCode == RESULT_OK && data != null && data.getData() != null) {
                draft.fileUri = data.getData();
                draft.fileName = requestFileName(draft.fileUri);
            }
            openManualRequest(draft);
            return;
        }
        if (requestCode != SAVE_CA_REQUEST || resultCode != RESULT_OK || data == null || data.getData() == null || certificatePem == null) return;
        final String pem = certificatePem;
        certificatePem = null;
        worker.execute(() -> {
            try (OutputStream output = getContentResolver().openOutputStream(data.getData())) {
                if (output == null) throw new IllegalStateException("无法打开目标文件");
                output.write(pem.getBytes(StandardCharsets.UTF_8));
                main.post(() -> {
                    status.setText("CA 公钥证书已保存");
                    new AlertDialog.Builder(this)
                        .setTitle("下一步：在系统设置中安装 CA")
                        .setMessage("CA 公钥证书已保存。请在系统设置中搜索“安装证书”或“CA 证书”，选择刚保存的文件并按系统提示确认。Android 11 及以上必须通过系统设置安装 CA；部分应用不信任用户 CA，或使用证书固定，仍无法解密其 HTTPS。")
                        .setPositiveButton("知道了", null)
                        .show();
                });
            } catch (Exception error) {
                main.post(() -> status.setText("保存 CA 失败：" + error.getMessage()));
            }
        });
    }

    private static JSONObject parseReply(String raw) {
        try { return new JSONObject(raw); }
        catch (JSONException | NullPointerException error) {
            JSONObject failure = new JSONObject();
            try { failure.put("ok", false); failure.put("error", "原生内核返回无效数据"); }
            catch (JSONException ignored) { }
            return failure;
        }
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        state.putBoolean("vpnPermissionPending", vpnPermissionPending);
        state.putInt("pendingVpnPort", pendingVpnPort);
        if (pendingFileRequest != null) {
            state.putBoolean("requestFilePending", true);
            state.putString("requestMethod", pendingFileRequest.method);
            state.putString("requestUrl", pendingFileRequest.url);
            state.putString("requestHeaders", pendingFileRequest.headers);
            state.putString("requestBody", pendingFileRequest.body);
            state.putString("requestEngine", pendingFileRequest.engine);
            state.putInt("requestBodyMode", pendingFileRequest.bodyMode);
            state.putString("requestFileUri", pendingFileRequest.fileUri == null
                ? null : pendingFileRequest.fileUri.toString());
            state.putString("requestFileName", pendingFileRequest.fileName);
        }
        super.onSaveInstanceState(state);
    }

    @Override protected void onDestroy() {
        running = false;
        main.removeCallbacksAndMessages(null);
        worker.shutdown();
        super.onDestroy();
    }
}
