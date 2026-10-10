import { useState } from "react";
import { decodeText, encodeText, type RequestDraft } from "./types";
import { t } from "./i18n";
type Part = { name: string; value: string; file?: File };
export default function RequestBodyEditor({
  draft,
  onChange,
}: {
  draft: RequestDraft;
  onChange: (draft: RequestDraft) => void;
}) {
  const [mode, setMode] = useState("text");
  const [parts, setParts] = useState<Part[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  function setBody(bodyBase64: string, mime?: string) {
    let found = false;
    const headers = draft.headers
      .filter(
        (h) =>
          !["content-length", "transfer-encoding"].includes(
            h.name.toLowerCase(),
          ),
      )
      .map((h) => {
        if (mime && h.name.toLowerCase() === "content-type") {
          found = true;
          return { ...h, value: mime };
        }
        return h;
      });
    if (mime && !found) headers.push({ name: "Content-Type", value: mime });
    onChange({ ...draft, headers, bodyBase64 });
  }
  function choose(value: string) {
    setMode(value);
    setError("");
    if (value === "form") {
      const values = new URLSearchParams(decodeText(draft.bodyBase64));
      setParts([...values].map(([name, value]) => ({ name, value })));
    } else if (value === "multipart") setParts([]);
  }
  async function apply() {
    setBusy(true);
    setError("");
    try {
      if (mode === "form") {
        setBody(
          encodeText(
            new URLSearchParams(parts.map((p) => [p.name, p.value])).toString(),
          ),
          "application/x-www-form-urlencoded",
        );
        return;
      }
      if (
        parts.reduce(
          (n, p) =>
            n +
            (p.file?.size ?? new TextEncoder().encode(p.value).length) +
            new TextEncoder().encode(p.name).length,
          0,
        ) >
        8 * 1024 * 1024
      )
        throw Error(t("请求正文不能超过 8 MiB"));
      const data = new FormData();
      for (const p of parts) {
        if (p.file) data.append(p.name, p.file, p.file.name);
        else data.append(p.name, p.value);
      }
      const request = new Request("http://localhost/", {
        method: "POST",
        body: data,
      });
      const bytes = new Uint8Array(await request.arrayBuffer());
      if (bytes.length > 8 * 1024 * 1024)
        throw Error(t("请求正文不能超过 8 MiB"));
      let binary = "";
      for (let i = 0; i < bytes.length; i += 32768)
        binary += String.fromCharCode(...bytes.subarray(i, i + 32768));
      setBody(btoa(binary), request.headers.get("content-type")!);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <fieldset
      disabled={busy}
      className="body-editor"
      style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}
    >
      <div className="editor-toolbar">
        <span>{t("请求正文")}</span>
        <select
          aria-label={t("正文编码")}
          value={mode}
          onChange={(e) => choose(e.target.value)}
        >
          <option value="text">UTF-8</option>
          <option value="base64">Base64</option>
          <option value="form">x-www-form-urlencoded</option>
          <option value="multipart">multipart/form-data</option>
        </select>
      </div>
      {mode === "text" || mode === "base64" ? (
        <textarea
          aria-label={t("请求正文")}
          spellCheck={false}
          value={
            mode === "base64" ? draft.bodyBase64 : decodeText(draft.bodyBase64)
          }
          onChange={(e) =>
            setBody(
              mode === "base64" ? e.target.value : encodeText(e.target.value),
            )
          }
        />
      ) : (
        <div className="request-form">
          <p>
            {t(
              "表单修改点击应用后写入正文；Multipart 编辑器用于重新构造正文，原正文保留到应用时。",
            )}
          </p>
          {parts.map((part, index) => (
            <div className="library-toolbar" key={index}>
              <input
                aria-label={t("字段名")}
                value={part.name}
                onChange={(e) =>
                  setParts((v) =>
                    v.map((p, i) =>
                      i === index ? { ...p, name: e.target.value } : p,
                    ),
                  )
                }
              />
              <input
                aria-label={t("字段值")}
                disabled={!!part.file}
                value={part.file?.name ?? part.value}
                onChange={(e) =>
                  setParts((v) =>
                    v.map((p, i) =>
                      i === index ? { ...p, value: e.target.value } : p,
                    ),
                  )
                }
              />
              {mode === "multipart" && (
                <input
                  aria-label={t("上传文件")}
                  type="file"
                  onChange={(e) => {
                    const file = e.target.files?.[0];
                    setParts((v) =>
                      v.map((p, i) => (i === index ? { ...p, file } : p)),
                    );
                  }}
                />
              )}
              <button
                onClick={() => setParts((v) => v.filter((_, i) => i !== index))}
              >
                {t("移除")}
              </button>
            </div>
          ))}
          <button onClick={() => setParts([...parts, { name: "", value: "" }])}>
            {t("添加字段")}
          </button>
          <button disabled={busy} onClick={() => void apply()}>
            {t("应用表单正文")}
          </button>
        </div>
      )}
      {error && <p role="alert">{error}</p>}
    </fieldset>
  );
}
