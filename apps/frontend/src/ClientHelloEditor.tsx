import { useState } from "react";
import { t } from "./i18n";
import type { TlsProfile } from "./types";
const key = "http-capture.clienthello-templates.v1";
type Template = { name: string; hex: string };
function read(): Template[] {
  try {
    const v = JSON.parse(localStorage.getItem(key) || "[]");
    return Array.isArray(v)
      ? v.filter((x) => typeof x.name === "string" && typeof x.hex === "string")
      : [];
  } catch {
    return [];
  }
}
export default function ClientHelloEditor({
  tls,
  onChange,
}: {
  tls: TlsProfile;
  onChange: (tls: TlsProfile) => void;
}) {
  const [templates, setTemplates] = useState(read);
  const [name, setName] = useState("");
  const [error, setError] = useState("");
  const enabled = tls.clientHelloHex != null;
  function save() {
    const hex = (tls.clientHelloHex || "").replace(/\s/g, "");
    if (
      !name.trim() ||
      !/^16[0-9a-f]+$/i.test(hex) ||
      hex.length % 2 ||
      hex.length < 18 ||
      hex.length > 131080 ||
      parseInt(hex.slice(6, 10), 16) !== hex.length / 2 - 5 ||
      hex.slice(10, 12) !== "01"
    ) {
      setError(t("请输入名称和完整的 ClientHello Hex"));
      return;
    }
    const next = [
      ...templates.filter((x) => x.name !== name.trim()),
      { name: name.trim(), hex },
    ];
    try {
      localStorage.setItem(key, JSON.stringify(next));
      setTemplates(next);
      setError("");
    } catch {
      setError(t("保存失败"));
    }
  }
  return (
    <section className="clienthello-editor">
      <label>
        <input
          type="checkbox"
          checked={enabled}
          onChange={(e) =>
            onChange({ ...tls, clientHelloHex: e.target.checked ? "" : null })
          }
        />
        {t("导入 ClientHello Hex")}
      </label>
      <p>
        {t(
          "TLS 使用导入模板；HTTP/2 使用上方浏览器预设。生成新的密钥与目标 SNI，不是原始握手字节重放。",
        )}
      </p>
      {enabled && (
        <>
          <select
            aria-label={t("ClientHello 模板")}
            value=""
            onChange={(e) => {
              const v = templates.find((x) => x.name === e.target.value);
              if (v) {
                setName(v.name);
                onChange({ ...tls, clientHelloHex: v.hex });
              }
            }}
          >
            <option value="">{t("选择已保存模板")}</option>
            {templates.map((x) => (
              <option key={x.name}>{x.name}</option>
            ))}
          </select>
          <textarea
            aria-label="ClientHello Hex"
            spellCheck={false}
            placeholder="16 03 01 ..."
            value={tls.clientHelloHex || ""}
            onChange={(e) =>
              onChange({ ...tls, clientHelloHex: e.target.value })
            }
          />
          <div>
            <input
              aria-label={t("模板名称")}
              placeholder={t("模板名称")}
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
            <button onClick={save}>{t("保存模板")}</button>
          </div>
          <small>
            {t(
              "粘贴完整 TLS record，支持空格和换行。当前不支持 ECH、PSK、0-RTT、QUIC 及未知扩展；发送前会校验。模板保存在当前设备，请求收藏也会保留 Hex。",
            )}
          </small>
        </>
      )}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
