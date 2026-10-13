/** Format token boundaries without modifying quoted strings or comments. */
export function formatProto(text:string):string {
 if(text.trim().startsWith('{'))return JSON.stringify(JSON.parse(text),null,2);
 const tokens=text.match(/"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|\/\/[^\n]*|\/\*[\s\S]*?\*\/|[A-Za-z0-9_.$+-]+|[^\s]/g)??[];
 let depth=0,line='';const lines:string[]=[];
 const flush=()=>{if(line.trim())lines.push('  '.repeat(depth)+line.trim());line=''};
 for(const token of tokens){if(token==='}'){flush();depth=Math.max(0,depth-1);line='}';flush()}else if(token==='{'){line+=' {';flush();depth++}else if(token===';'){line+=';';flush()}else if(token.startsWith('//')||token.startsWith('/*')){flush();line=token;flush()}else line+=(line?' ':'')+token;}
 flush();return lines.join('\n')+'\n';
}
