import { t, currentLanguage } from './i18n';
import type { Flow, RequestDraft, TlsDetailsData } from './types';

export function PropertyTable({ rows }: { rows: Array<[string, string | number]> }) {
  return <table className="property-table"><thead><tr><th>{t("名称")}</th><th>{t("值")}</th></tr></thead><tbody>{rows.map(([name, value]) => <tr key={name}><td>{name}</td><td>{value}</td></tr>)}</tbody></table>;
}
function size(body: string) { return `${body.length * 3 / 4 - (body.endsWith('==') ? 2 : body.endsWith('=') ? 1 : 0)} B`; }
export function RequestOverview({ request, flow }: { request: RequestDraft; flow?: Flow }) {
  return <div className="request-details">
    <PropertyTable rows={[
      ['URL', request.url || t("未填写")], [t("状态"), flow ? flow.error ? t("失败") : flow.response ? t("已完成") : t("处理中") : t("未发送")],
      [t("方法"), request.method], [t("客户端协议"), flow?.clientProtocol ?? t("未记录")],
      [t("上游协议"), flow?.response?.version ?? t("未记录")], [t("响应状态码"), flow?.response?.status ?? '—'],
      [t("发送引擎"), request.engine ?? 'auto'],
    ]} />
    <details open><summary>{t("时间")}</summary><PropertyTable rows={[
      [t("开始时间"), flow ? new Date(flow.startedAt).toLocaleString(currentLanguage()) : t("未发送")], [t("总时长"), flow ? `${flow.durationMs} ms` : '—'],
    ]} /></details>
    <details open><summary>{t("正文大小")}</summary><PropertyTable rows={[
      [t("请求"), size(request.bodyBase64)], [t("响应"), flow?.response ? size(flow.response.bodyBase64) : '—'],
    ]} /></details>
    {flow?.error && <p className="error-box">{flow.error}</p>}
  </div>;
}
function TlsConnection({ data, title, empty }: { data?: TlsDetailsData | null; title: string; empty: string }) {
  return <details open><summary>{title}</summary>{!data ? <p className="detail-note">{empty}</p> : <>
    <PropertyTable rows={[
      [t("版本"), data.version ?? t("未记录")], [t("协商密码套件"), data.cipherSuite ?? t("未记录")],
      [t("协商 ALPN"), data.alpn ?? t("未协商")], ['SNI', data.serverName ?? t("未提供 / 未记录")],
      [t("握手类型"), data.handshakeKind ?? t("未记录")],
    ]} />
    {([[t("ClientHello · 提供的密码套件"), data.offeredCipherSuites], ['ClientHello · ALPN', data.offeredAlpn],
      [t("ClientHello · 签名算法"), data.signatureSchemes], [t("ClientHello · 支持的组"), data.supportedGroups]] as Array<[string, string[]]>).map(([label, values]) => values.length > 0 && <details key={label}><summary>{label}（{values.length}）</summary><PropertyTable rows={values.map((value, i) => [String(i + 1), value])} /></details>)}
    <details open><summary>{t("服务端证书链（")}{data.certificates.length}）</summary>
      {data.certificates.length === 0 && <p className="detail-note">{t("未记录证书。")}</p>}
      {data.certificates.map((cert, index) => <details key={index} open={index === 0}><summary>{index === 0 ? t("叶证书") : t("证书 {v0}", { v0: index + 1 })} · {cert.subject || t("解析失败")}</summary>
        <PropertyTable rows={[
          ['Subject', cert.subject], [t("签发者"), cert.issuer], [t("序列号"), cert.serial],
          [t("生效时间"), cert.notBefore], [t("到期时间"), cert.notAfter], [t("SHA-256 指纹"), cert.sha256],
          ['SAN', cert.dnsNames.join('\n') || '—'],
        ]} />
        {cert.parseError && <p className="detail-note">{t("证书解析失败：")}{cert.parseError}</p>}
        <a className="certificate-download" download={`certificate-${index + 1}.pem`} href={'data:application/x-pem-file;charset=utf-8,' + encodeURIComponent('-----BEGIN CERTIFICATE-----\n' + (cert.derBase64.match(/.{1,64}/g) ?? []).join('\n') + '\n-----END CERTIFICATE-----\n')}>{t("下载 PEM 证书")}</a>
      </details>)}
    </details>
  </>}</details>;
}
export function TlsDetails({ flow }: { flow?: Flow }) {
  const secure = flow?.request.url.startsWith('https:');
  const empty = !flow ? t("尚未发送") : !secure ? t("HTTP 明文连接，无 TLS 握手") : t("此记录没有握手详情，请重新发送或捕获。");
  return <div className="request-details">
    <TlsConnection title={t("客户端 → 代理")} data={flow?.clientTls} empty={flow?.source === 'replay' ? t("主动发送请求，不存在客户端到代理的握手。") : empty} />
    <TlsConnection title={t("代理 → 目标服务器")} data={flow?.response?.upstreamTls} empty={empty} />
    {flow?.source === 'capture' && secure && <p className="detail-note">{t("客户端侧显示代理签发的拦截证书；上游侧显示目标服务器实际提供的证书链。")}</p>}
  </div>;
}
