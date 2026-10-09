export interface Header { name: string; value: string }
export interface TlsProfile {
  browserVersion?: string | null;
  version?: string | null;
  preset: string;
  cipherList?: string | null;
  sigalgsList?: string | null;
  curvesList?: string | null;
  grease?: boolean | null;
  permuteExtensions?: boolean | null;
}
export interface ScriptModules { encoding:boolean;crypto:boolean;utils:boolean }
export interface Scripts { modules?:ScriptModules; enabled: boolean; before: string; after: string; variables: Record<string,string> }
export interface RequestDraft { upstreamProfileId?:string|null; scripts?: Scripts; pseudoHeaders?: Header[]; engine?: string; method: string; url: string; headers: Header[]; bodyBase64: string; tls: TlsProfile }
export interface CertificateDetails { subject: string; issuer: string; serial: string; notBefore: string; notAfter: string; sha256: string; dnsNames: string[]; derBase64: string; parseError?: string | null }
export interface TlsDetailsData { version?: string | null; cipherSuite?: string | null; alpn?: string | null; serverName?: string | null; handshakeKind?: string | null; offeredCipherSuites: string[]; offeredAlpn: string[]; signatureSchemes: string[]; supportedGroups: string[]; certificates: CertificateDetails[] }
export interface Flow {
  websocket?: { state: string; frames: { direction: string; atMs: number; opcode: number; fin: boolean; compressed: boolean; payloadBase64: string }[] } | null;
  originalRequest?: RequestDraft | null;
  originalResponse?: Flow['response'];
  clientTls?: TlsDetailsData | null;
  clientProtocol?: string | null;
  id: string; parentId: string | null; startedAt: number; durationMs: number; source: string;
  request: RequestDraft;
  response: { status: number; version: string; tlsVersion?: string | null; upstreamTls?: TlsDetailsData | null; sentRequestHeaders?: Header[] | null; headers: Header[]; bodyBase64: string; rawHeadBase64?: string | null } | null;
  error: string | null; rawRequestHeadBase64?: string | null; notes: string[];
}
export interface EngineInfo { browserVersions?: Record<string, number[]>; id: string; available: boolean; profiles: string[]; reason: string | null }
export interface UpstreamProfile {id:string;name:string;url:string;username:string;authEnabled:boolean;needsPassword:boolean;rememberPassword:boolean;credentialError?:string|null}
export interface UpstreamStatus {profileId?:string|null;profileName?:string|null; enabled: boolean; url: string; username: string; authEnabled: boolean }
export interface ProxyStatus { upstream?: UpstreamStatus; running: boolean; address: string | null; caPath: string; browserReplay: boolean; sendEngines?: EngineInfo[] }
export function encodeText(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = ''; for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}
export function decodeText(value: string): string {
  try { return new TextDecoder('utf-8', { fatal: true }).decode(Uint8Array.from(atob(value), c => c.charCodeAt(0))); }
  catch { return '[二进制内容：请在 Base64 模式查看]'; }
}
