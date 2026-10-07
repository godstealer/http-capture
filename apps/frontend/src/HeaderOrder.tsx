import { t } from './i18n';
import type { Header } from './types';
import { PropertyTable } from './RequestDetails';

export default function HeaderOrder({ headers, sent, captured = false, original = true, onCopy }: {
  headers: Header[]; sent?: Header[] | null; captured?: boolean; original?: boolean; onCopy: (value: string) => void;
}) {
  function list(fields: Header[]) {
    return <><div className="order-copy-actions"><button className="text-button" disabled={!fields.length} onClick={() => onCopy(fields.map(h => h.name).join('\n'))}>{t("复制字段名")}</button><button className="text-button" disabled={!fields.length} onClick={() => onCopy(JSON.stringify(fields.map(h => h.name), null, 2))}>{t("复制 JSON 数组")}</button></div><PropertyTable rows={fields.map((h, i) => [`${i + 1}`, h.name || t("（空字段名）")])} /></>;
  }
  return <div className="header-order request-details">
    <details open><summary>{captured ? original ? t("客户端原始字段顺序") : t("解析后的字段列表（原始顺序未记录）") : t("编辑列表顺序")}</summary>
    <p className="detail-note">{captured && !original ? t("解析后的字段列表；未记录线上原始顺序。") : captured ? t("客户端原始字段顺序；HTTP/2 包含伪头部，保留 HPACK 解码后的顺序。") : t("在请求头列表中使用上移、下移调整顺序。")}</p>
    {list(headers)}
    </details><details open><summary>{t("最近一次发送 · 上游实际字段顺序")}</summary>
    {sent ? list(sent) : <p>{t("暂无记录。新捕获或发送的 HTTP/1、HTTP/2 请求成功后显示；旧记录和 HTTP/3 不提供此数据。")}</p>}
    </details>{sent && <details><summary>{captured && !original ? t("原始顺序未记录，无法核对保真") : JSON.stringify(headers) === JSON.stringify(sent) ? t("客户端/编辑列表与上游字段一致") : t("存在转发调整 · 查看逐项对照")}</summary><table className="property-table order-comparison"><thead><tr><th>{t("客户端 / 编辑列表")}</th><th>{t("上游实际发送")}</th></tr></thead><tbody>{Array.from({ length: Math.max(headers.length, sent.length) }, (_, i) => <tr key={i}><td>{headers[i] ? `${i + 1}. ${headers[i].name}: ${headers[i].value}` : '—'}</td><td>{sent[i] ? `${i + 1}. ${sent[i].name}: ${sent[i].value}` : '—'}</td></tr>)}</tbody></table></details>}<details><summary>{t("发送规则")}</summary><p className="detail-note">{t("HTTP/1 发送时会移除逐跳头和 Expect；Host、Content-Length 按实际目标和正文修正，已有字段位置不变。缺失的 Host 补在开头，Content-Length 按需补在末尾，最后追加 Connection: close。")}</p>
    <p className="detail-note">{t("抓包转发跟随客户端协议：HTTP/1 → HTTP/1，HTTP/2 → HTTP/2。HTTP/2 保留伪头部顺序及普通字段顺序，不自动补充 Host 或 Content-Length。主动发送仍可选择 Auto。字段保序不代表 HPACK 压缩字节、帧边界或 TLS 握手完全相同。")}</p></details>
  </div>;
}
