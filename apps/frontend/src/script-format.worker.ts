import { format } from 'prettier/standalone';
import * as babel from 'prettier/plugins/babel';
import * as estree from 'prettier/plugins/estree';
import type { Plugin } from 'prettier';
self.onmessage=async(event:MessageEvent<{text:string;parser:'babel'|'json'}>)=>{try{const text=await format(event.data.text,{parser:event.data.parser==='json'?'json-stringify':event.data.parser,plugins:[babel,estree as Plugin],tabWidth:2,printWidth:100});self.postMessage({text});}catch(error){self.postMessage({error:error instanceof Error?error.message:String(error)});}};
