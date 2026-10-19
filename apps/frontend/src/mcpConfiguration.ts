export type McpConfigurationClient='codex'|'cursor'|'vscode'|'generic';
export type McpConfigurationTransport='http'|'stdio';
export interface McpConnectionInfo {
  url:string;headers:{Authorization:string};
  stdio?:{command:string;args:string[];env:Record<string,string>};
}
export interface McpConfiguration {text:string;language:'json'|'toml';target:string;documentationUrl:string}
// JSON string literals are also valid TOML basic strings for these generated
// values. The command is an absolute application path; arguments stay separate.
const quoted=(value:string)=>JSON.stringify(value);
export function buildMcpConfiguration(client:McpConfigurationClient,transport:McpConfigurationTransport,connection:McpConnectionInfo):McpConfiguration {
  const url=new URL(connection.url);
  if(url.protocol!=='http:'||!['127.0.0.1','localhost'].includes(url.hostname)||url.pathname!=='/mcp'||url.username||url.password||url.search||url.hash)throw Error('Invalid local MCP endpoint');
  if(!/^Bearer [a-f0-9]{64}$/i.test(connection.headers?.Authorization))throw Error('Invalid MCP credential');
  let server:Record<string,unknown>;
  if(transport==='stdio'){
    const launch=connection.stdio;
    if(!launch||!launch.command||!Array.isArray(launch.args)||launch.args.some(value=>typeof value!=='string')||typeof launch.env?.HTTP_CAPTURE_MCP_TOKEN!=='string')throw Error('The running application does not provide stdio configuration');
    server={command:launch.command,args:launch.args,env:launch.env};
  }else server={url:connection.url,headers:connection.headers};
  if(client==='codex'){
    const lines=['[mcp_servers.httpCapture]'];
    if(transport==='http')lines.push(`url = ${quoted(connection.url)}`,`http_headers = { Authorization = ${quoted(connection.headers.Authorization)} }`);
    else{
      lines.push(`command = ${quoted(server.command as string)}`,`args = ${JSON.stringify(server.args)}`,'','[mcp_servers.httpCapture.env]');
      for(const [name,value] of Object.entries(connection.stdio!.env))lines.push(`${quoted(name)} = ${quoted(value)}`);
    }
    return {text:lines.join('\n')+'\n',language:'toml',target:'~/.codex/config.toml',documentationUrl:'https://learn.chatgpt.com/docs/extend/mcp?surface=cli'};
  }
  if(client==='vscode')return {text:JSON.stringify({servers:{httpCapture:{type:transport,...server}}},null,2),language:'json',target:'.vscode/mcp.json',documentationUrl:'https://code.visualstudio.com/docs/agents/reference/mcp-configuration'};
  if(client==='cursor')return {text:JSON.stringify({mcpServers:{httpCapture:server}},null,2),language:'json',target:'~/.cursor/mcp.json',documentationUrl:'https://prod.cursor.com/help/customization/mcp'};
  return {text:JSON.stringify(server,null,2),language:'json',target:'',documentationUrl:''};
}
