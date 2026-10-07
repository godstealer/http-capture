import { jsonLanguage } from '@codemirror/lang-json';
export function parseVariables(text:string):Record<string,string> {
 const tree=jsonLanguage.parser.parse(text);let failure='';const keys=new Set<string>();
 tree.iterate({enter(node){if(failure)return false;if(node.type.isError){const lines=text.slice(0,node.from).split('\n');failure=`JSON 语法错误：第 ${lines.length} 行，第 ${lines[lines.length-1].length+1} 列`;return false;}if(node.name==='PropertyName'){try{const key=JSON.parse(text.slice(node.from,node.to));if(keys.has(key))failure=`变量名重复：${key}`;keys.add(key);}catch{}}}});
 if(failure)throw new Error(failure);
 const value:unknown=JSON.parse(text);if(!value||typeof value!=='object'||Array.isArray(value))throw new Error('变量必须是 JSON 对象');
 if(Object.values(value).some(v=>typeof v!=='string'))throw new Error('每个变量值必须是字符串，例如 {"secret": "abc"}');
 if(new TextEncoder().encode(text).length>65536)throw new Error('变量不能超过 64 KiB');
 return value as Record<string,string>;
}
