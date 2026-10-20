import { invoke as tauriInvoke, isTauri } from '@tauri-apps/api/core';

export async function invoke<T = unknown>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri()) return tauriInvoke<T>(command, args);
  const routes: Record<string, [string, string]> = {
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
    network_interfaces: ['GET', 'network/interfaces'], check_listener_port: ['POST', 'network/check-port'],
    request_workspace:['GET','workspace'], save_request_workspace:['POST','workspace'], workspace_revisions:['GET','workspace/revisions'],
    config_status:['GET','config/status'],config_export:['POST','config/export'],config_preview:['POST','config/preview'],config_import:['POST','config/import'],config_recovery:['POST','config/recovery'],
    interception: ['GET', 'interception'], configure_interception: ['POST', 'interception'], resolve_interception: ['POST', 'interception/resolve'],
    blocking: ['GET', 'blocking'], configure_blocking: ['POST', 'blocking'],
    fault_rules:['GET','fault-rules'],configure_fault_rules:['POST','fault-rules'],mcp_audit:['POST','mcp/audit'],
    response_rewrite:['GET','response-rewrite'],configure_response_rewrite:['POST','response-rewrite'],
    flow_metadata_changes:['POST','flows/metadata/changes'],flow_metadata:['POST','flows/metadata'],get_flow:['POST','flows/get'],read_flow_body:['POST','flows/body/read'],
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
