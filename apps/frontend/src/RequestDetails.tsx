import { t, currentLanguage } from './i18n';
import type { FlowRecord, Header, RequestDraft, RequestMetadata, TlsDetailsData } from './types';
import { flowBodyLength } from './flowRecords';
import { Copy } from 'lucide-react';
import { headerLines } from './messageText';
import { describeTrafficOrigin } from './trafficOrigin';

export function MessageHeaders({ headers, onCopy }: { headers: Header[]; onCopy: (text: string) => void }) {
  return <div className="message-headers">
    <div className="message-copy-toolbar"><button type="button" disabled={!headers.length} onClick={() => onCopy(headerLines(headers))}><Copy size={13} />{t('复制全部头部')}</button></div>
    <div className="inspector-headers">{headers.map((header, index) => <div key={index}>
      <span>{index + 1}</span><strong>{header.name}</strong><code>{header.value}</code>
      <button type="button" className="icon-button" title={t('复制头部 {v0}', {v0:index + 1})} aria-label={t('复制头部 {v0}', {v0:index + 1})} onClick={() => onCopy(headerLines([header]))}><Copy size={13} /></button>
    </div>)}</div>
  </div>;
}

export function PropertyTable({ rows }: { rows: Array<[string, string | number]> }) {
  return <table className="property-table"><thead><tr><th>{t("名称")}</th><th>{t("值")}</th></tr></thead><tbody>{rows.map(([name, value]) => <tr key={name}><td>{name}</td><td>{value}</td></tr>)}</tbody></table>;
}
function size(length: number | undefined) { return length === undefined ? t('未加载') : `${length} B`; }
export function TrafficOriginDetails({ flow }: { flow: FlowRecord }) {
  const origin = describeTrafficOrigin(flow);
  return <details open className="traffic-origin-details"><summary>{t('流量来源')}{origin.imported && <span className="traffic-origin-file-badge">{t('文件记录')}</span>}</summary>
    <PropertyTable rows={[
      [t('来源应用'), origin.application], ['PID', origin.pid], [t('进程路径'), origin.path],
      [t('客户端地址'), origin.clientAddress], [t('流量入口'), origin.entry], [t('识别说明'), origin.reason],
    ]} />
    {origin.provenance && <p className="traffic-origin-provenance">{origin.provenance}</p>}
  </details>;
}
export function RequestOverview({ request, flow }: { request: RequestDraft | RequestMetadata; flow?: FlowRecord }) {
  const requestLength = 'bodyBase64' in request ? request.bodyBase64.length * 3 / 4 - (request.bodyBase64.endsWith('==') ? 2 : request.bodyBase64.endsWith('=') ? 1 : 0) : flow ? flowBodyLength(flow, request === flow.originalRequest ? 'originalRequest' : 'request') : undefined;
  return <div className="request-details">
    <PropertyTable rows={[
      ['URL', request.url || t("未填写")], [t("状态"), flow?.state === 'blocked' ? t('已阻止') : flow ? flow.error ? t("失败") : flow.response ? t("已完成") : t("处理中") : t("未发送")],
      [t("方法"), request.method], [t("客户端协议"), flow?.clientProtocol ?? t("未记录")],
      [t("上游协议"), flow?.state === 'blocked' ? t('本地阻止') : flow?.response?.version ?? t("未记录")], [t("响应状态码"), flow?.response?.status ?? '—'],
      [t("发送引擎"), request.engine ?? 'auto'],
    ]} />
    {flow && <TrafficOriginDetails flow={flow} />}
    <details open><summary>{t("时间")}</summary><PropertyTable rows={[
      [t("开始时间"), flow ? new Date(flow.startedAt).toLocaleString(currentLanguage()) : t("未发送")], [t("总时长"), flow ? `${flow.durationMs} ms` : '—'],
    ]} /></details>
    <details open><summary>{t("正文大小")}</summary><PropertyTable rows={[
      [t("请求"), size(requestLength)], [t("响应"), flow?.response ? size(flowBodyLength(flow, 'response')) : '—'],
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
export function TlsDetails({ flow }: { flow?: FlowRecord }) {
  const secure = flow?.request.url.startsWith('https:');
  const empty = !flow ? t("尚未发送") : !secure ? t("HTTP 明文连接，无 TLS 握手") : t("此记录没有握手详情，请重新发送或捕获。");
  return <div className="request-details">
    <TlsConnection title={t("客户端 → 代理")} data={flow?.clientTls} empty={flow?.source === 'replay' ? t("主动发送请求，不存在客户端到代理的握手。") : empty} />
    <TlsConnection title={t("代理 → 目标服务器")} data={flow?.response?.upstreamTls} empty={empty} />
    {flow?.source === 'capture' && secure && <p className="detail-note">{t("客户端侧显示代理签发的拦截证书；上游侧显示目标服务器实际提供的证书链。")}</p>}
  </div>;
}
