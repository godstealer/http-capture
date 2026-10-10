import { useEffect, useState } from "react";
import { invoke } from "./api";
import { t } from "./i18n";
export default function DecryptionSettings() {
  const [hosts, setHosts] = useState("");
  const [ready, setReady] = useState(false);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let live = true;
    invoke<string[]>("decryption_hosts")
      .then((v) => {
        if (live) {
          setHosts(v.join("\n"));
          setReady(true);
        }
      })
      .catch((e) => {
        if (live) setMessage(String(e));
      });
    return () => {
      live = false;
    };
  }, []);
  async function save() {
    setBusy(true);
    try {
      await invoke("save_decryption_hosts", {
        hosts: hosts
          .split(/\r?\n/)
          .map((v) => v.trim())
          .filter(Boolean),
      });
      setMessage(t("已保存，下一条连接生效"));
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <fieldset disabled={!ready || busy}>
      <legend>{t("HTTPS 解密例外")}</legend>
      <p>
        {t(
          "每行一个域名，支持 *.example.com。匹配的显式 CONNECT 连接直接转发 TLS，不解密内部流量；仍使用当前上游代理。空列表表示全部解密。",
        )}
      </p>
      <textarea
        aria-label={t("不解密域名")}
        value={hosts}
        onChange={(e) => setHosts(e.target.value)}
        rows={4}
      />
      <button onClick={() => void save()}>{t("保存解密策略")}</button>
      <p role="status">{message}</p>
    </fieldset>
  );
}
