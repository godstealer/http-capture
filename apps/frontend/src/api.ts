import { invoke as tauriInvoke, isTauri } from '@tauri-apps/api/core';
import type {BodyVariant} from './types';

export async function invoke<T = unknown>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri()) return tauriInvoke<T>(command, args);
  const routes: Record<string, [string, string]> = {
    certificates:['GET','certificates'],import_certificate:['POST','certificates/import'],change_certificate:['POST','certificates/change'],
    mcp_status:['GET','mcp/status'],mcp_configure:['POST','mcp/configure'],mcp_connection_info:['POST','mcp/connection'],
    mcp_clients:['GET','mcp/clients'],mcp_save_client:['POST','mcp/clients/save'],mcp_remove_client:['POST','mcp/clients/remove'],
    mcp_connect_client:['POST','mcp/clients/connect'],mcp_disconnect_client:['POST','mcp/clients/disconnect'],mcp_client_tools:['POST','mcp/clients/tools'],mcp_call_tool:['POST','mcp/clients/call'],
    plugins_validate:['POST','plugins/validate'],
    plugins_list:['GET','plugins'],plugins_save:['POST','plugins'],plugin_logs:['GET','plugins/logs'],
    grpc_poll:['POST','grpc/poll'],grpc_start:['POST','grpc/start'],grpc_send:['POST','grpc/send'],
    system_proxy_status:['GET','system-proxy'], set_system_proxy:['POST','system-proxy'], ca_status:['GET','ca/trust'], set_ca_trust:['POST','ca/trust'],
    websocket_connect:['POST','websocket/connect'],websocket_send:['POST','websocket/send'],
    decryption_hosts:['GET','decryption'],save_decryption_hosts:['POST','decryption'],test_upstream:['POST','upstream/test'],
    flow_changes:['POST','flows/changes'],
    request_library:['GET','library'], save_request_library:['POST','library'],
    backup_database:['POST','database/backup'], compact_database:['POST','database/compact'], clear_database:['POST','database/clear'],
    begin_import:['POST','imports/begin'], append_import:['POST','imports/append'], finish_import:['POST','imports/finish'],
    delete_flows:['POST','flows/delete'], import_flows:['POST','flows/import'],
    tun_status:['GET','tun'], start_tun:['POST','tun/start'], stop_tun:['POST','tun/stop'],
    reverse_quic_status:['GET','quic-reverse'], start_reverse_quic:['POST','quic-reverse/start'], stop_reverse_quic:['POST','quic-reverse/stop'],
    network_interfaces: ['GET', 'network/interfaces'], check_listener_port: ['POST', 'network/check-port'],
    request_workspace:['GET','workspace'], save_request_workspace:['POST','workspace'], workspace_revisions:['GET','workspace/revisions'],
    config_status:['GET','config/status'],config_export:['POST','config/export'],config_preview:['POST','config/preview'],config_import:['POST','config/import'],config_recovery:['POST','config/recovery'],
    interception: ['GET', 'interception'], configure_interception: ['POST', 'interception'], resolve_interception: ['POST', 'interception/resolve'],
    blocking: ['GET', 'blocking'], configure_blocking: ['POST', 'blocking'],
    fault_rules:['GET','fault-rules'],configure_fault_rules:['POST','fault-rules'],mcp_audit:['POST','mcp/audit'],
    response_rewrite:['GET','response-rewrite'],configure_response_rewrite:['POST','response-rewrite'],
    flow_metadata_changes:['POST','flows/metadata/changes'],flow_metadata:['POST','flows/metadata'],get_flow:['POST','flows/get'],read_flow_body:['POST','flows/body/read'],
    begin_browser_session_export:['POST','session-export/begin'],append_browser_session_export:['POST','session-export/append'],finish_browser_session_export:['POST','session-export/finish'],
    capture_scripts: ['GET', 'scripts'], set_capture_scripts: ['POST', 'scripts'],
    upstream_profiles: ['GET','upstream/profiles'], save_upstream_profile:['POST','upstream/profiles'], select_upstream_profile:['POST','upstream/select'], delete_upstream_profile:['POST','upstream/delete'],
    set_upstream: ['POST', 'upstream'], proxy_status: ['GET', 'status'], list_flows: ['GET', 'flows'], flow_revision: ['GET', 'flows/revision'],
    start_proxy: ['POST', 'start'], stop_proxy: ['POST', 'stop'],
    prepare_replay: ['POST', 'replay/prepare'], cancel_replay: ['POST', 'replay/cancel'], replay_request: ['POST', 'replay'], export_certificate: ['GET', 'certificate'],
  };
  const route = routes[command];
  if (!route) throw new Error(`Unknown command: ${command}`);
  const response = await fetch(`/__capture/${route[1]}`, { method: route[0],
    headers: { 'X-Capture-UI': '1', 'Content-Type': 'application/json' },
    body: route[0] === 'POST' ? JSON.stringify(args ?? {}) : undefined,
    signal: AbortSignal.timeout(command === 'replay_request' ? 360000 : command === 'mcp_call_tool' ? 70000 : command === 'mcp_connect_client' ? 45000 : ['start_tun','backup_database','compact_database','finish_import','config_import'].includes(command) ? 300000 : 35000),
  });
  if (!response.ok) throw new Error(`HTTP ${response.status}: ${await response.text()}`);
  return command === 'export_certificate' ? await response.text() as T : response.json();
}

export interface FlowBodyDownload {id:string;variant:BodyVariant;expectedVersion:string}
export interface FlowBodyDownloadResult {kind:'saved'|'browser'|'cancelled';name?:string;buffered:boolean}
interface BodyFileHandle {
  name:string;
  createWritable():Promise<{write(data:Uint8Array):Promise<void>;close():Promise<void>;abort():Promise<void>}>;
}
const MAX_BODY_DOWNLOAD=256*1024*1024;

/** Download the selected immutable entity, never the currently visible prefix.
 * File System Access writes with backpressure. The Blob compatibility path is
 * explicitly reported because that browser still buffers the complete file. */
export async function downloadFlowBody(args:FlowBodyDownload,options:{signal?:AbortSignal;expectedBytes?:number}={}):Promise<FlowBodyDownloadResult> {
  if(typeof args.id!=='string'||!args.id||args.id.length>128||typeof args.expectedVersion!=='string'||!args.expectedVersion||args.expectedVersion.length>128
    ||!['request','response','originalRequest','originalResponse'].includes(args.variant))throw Error('Invalid recorded body reference');
  if(options.expectedBytes!==undefined&&(!Number.isSafeInteger(options.expectedBytes)||options.expectedBytes<0||options.expectedBytes>MAX_BODY_DOWNLOAD))throw Error('Body download exceeds 256 MiB limit');
  const checkCancelled=()=>{if(options.signal?.aborted)throw new DOMException('Download cancelled','AbortError')};
  checkCancelled();
  if(isTauri()) {
    const name=await tauriInvoke<string>('save_flow_body',{id:args.id,variant:args.variant,expectedVersion:args.expectedVersion});
    checkCancelled();return {kind:'saved',name,buffered:false};
  }
  const filename=`http-capture-${args.variant}.bin`;
  const picker=(window as unknown as {showSaveFilePicker?:(options:{suggestedName:string})=>Promise<BodyFileHandle>}).showSaveFilePicker;
  let handle:BodyFileHandle|undefined;
  if(picker) {
    try {handle=await picker.call(window,{suggestedName:filename})}
    catch(error) {if((error as DOMException).name==='AbortError')return {kind:'cancelled',buffered:false};throw error}
  }
  checkCancelled();
  const controller=new AbortController(),abort=()=>controller.abort(options.signal?.reason);
  options.signal?.addEventListener('abort',abort,{once:true});
  const timeout=setTimeout(()=>controller.abort(new DOMException('Download timed out','TimeoutError')),300000);
  let response:Response|undefined;
  try {
    if(!handle){
      const prepared=await fetch('/__capture/flows/body/prepare',{method:'POST',redirect:'error',headers:{'X-Capture-UI':'1','Content-Type':'application/json'},
        body:JSON.stringify({id:args.id,variant:args.variant,expectedVersion:args.expectedVersion}),signal:controller.signal});
      // Older capture services still expose the authenticated POST download.
      if(prepared.status!==404){
        if(!prepared.ok)throw Error(`HTTP ${prepared.status}: ${(await prepared.text()).slice(0,2048)}`);
        const ticket=await prepared.json() as {url?:unknown;length?:unknown};
        if(typeof ticket.url!=='string'||!/^http:\/\/127\.0\.0\.1:1421\/session-export\/download\/[a-f0-9]{64}$/.test(ticket.url))throw Error('Invalid recorded body download URL');
        try {
          if(typeof ticket.length!=='number'||!Number.isSafeInteger(ticket.length)||ticket.length<0||ticket.length>MAX_BODY_DOWNLOAD
            ||options.expectedBytes!==undefined&&ticket.length!==options.expectedBytes)throw Error('Recorded body download length differs from its metadata');
          checkCancelled();
          const link=document.createElement('a');link.href=ticket.url;link.download=filename;link.referrerPolicy='no-referrer';link.click();
          return {kind:'browser',buffered:false};
        } catch(error) {
          await fetch('/__capture/session-export/finish',{method:'POST',headers:{'X-Capture-UI':'1','Content-Type':'application/json'},
            body:JSON.stringify({id:ticket.url.slice(-64),commit:false}),signal:AbortSignal.timeout(5000)}).catch(()=>{});
          throw error;
        }
      }
    }
    response=await fetch('/__capture/flows/body/download',{method:'POST',redirect:'error',headers:{'X-Capture-UI':'1','Content-Type':'application/json'},
      body:JSON.stringify({id:args.id,variant:args.variant,expectedVersion:args.expectedVersion}),signal:controller.signal});
    if(!response.ok)throw Error(`HTTP ${response.status}: ${(await response.text()).slice(0,2048)}`);
    const declared=response.headers.get('content-length');
    const total=declared!==null&&/^\d+$/.test(declared)?Number(declared):NaN;
    if(!Number.isSafeInteger(total)||total<0||total>MAX_BODY_DOWNLOAD||options.expectedBytes!==undefined&&total!==options.expectedBytes)throw Error('Recorded body download length differs from its metadata');
    if(handle) {
      if(!response.body)throw Error('Recorded body download stream is unavailable');
      const reader=response.body.getReader();let writer:Awaited<ReturnType<BodyFileHandle['createWritable']>>|undefined;
      try {
        writer=await handle.createWritable();let received=0;
        while(true) {
          checkCancelled();const {done,value}=await reader.read();if(done)break;
          received+=value.byteLength;
          if(received>total)throw Error('Recorded body download exceeds its declared length');
          await writer.write(value);
        }
        if(received!==total)throw Error('Incomplete recorded body download');
        checkCancelled();await writer.close();
        return {kind:'saved',name:handle.name,buffered:false};
      } catch(error) {await reader.cancel().catch(()=>{});await writer?.abort().catch(()=>{});throw error}
      finally {reader.releaseLock()}
    }
    const blob=await response.blob();checkCancelled();
    if(blob.size!==total)throw Error('Incomplete recorded body download');
    const url=URL.createObjectURL(blob);
    try {const link=document.createElement('a');link.href=url;link.download=filename;link.click()}
    finally {setTimeout(()=>URL.revokeObjectURL(url),30000)}
    return {kind:'browser',buffered:true};
  } catch(error) {
    controller.abort();await response?.body?.cancel().catch(()=>{});throw error;
  } finally {clearTimeout(timeout);options.signal?.removeEventListener('abort',abort)}
}
