import {useEffect, useRef} from 'react';
import {Compartment, EditorState} from '@codemirror/state';
import {EditorView, lineNumbers, keymap} from '@codemirror/view';
import {history, historyKeymap, defaultKeymap, indentWithTab} from '@codemirror/commands';
import {StreamLanguage, foldGutter, foldKeymap, syntaxHighlighting, defaultHighlightStyle, bracketMatching} from '@codemirror/language';
import {autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap} from '@codemirror/autocomplete';
import {json} from '@codemirror/lang-json';
import {javascript} from '@codemirror/lang-javascript';
import {html} from '@codemirror/lang-html';
import {css} from '@codemirror/lang-css';
import {t, useLanguage} from './i18n';
const protoLanguage=StreamLanguage.define({startState:()=>({comment:false}),token(stream,state){if(state.comment){if(stream.skipTo('*/')){stream.match('*/');state.comment=false}else stream.skipToEnd();return 'comment'}if(stream.eatSpace())return null;if(stream.match('//')){stream.skipToEnd();return 'comment'}if(stream.match('/*')){state.comment=true;return 'comment'}if(stream.match(/"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'/))return 'string';if(stream.match(/\b(?:syntax|package|import|message|enum|service|rpc|returns|stream|option|repeated|optional|required|oneof|map|reserved|extensions|extend)\b/))return 'keyword';if(stream.match(/\b(?:string|bytes|bool|double|float|s?fixed(?:32|64)|s?int(?:32|64)|uint(?:32|64))\b/))return 'typeName';if(stream.match(/-?\d+(?:\.\d+)?/))return 'number';stream.next();return null}});
const language = (parser:string) => parser==='proto'?protoLanguage:parser==='json'?json():parser==='babel'?javascript():parser==='html'?html():parser==='css'?css():[];
export default function BodyCodeEditor({value,parser,onChange,disabled=false,label}:{label?:string;value:string;parser:string;onChange:(text:string)=>void;disabled?:boolean}) {
 const parent=useRef<HTMLDivElement>(null), view=useRef<EditorView>();
 const config=useRef(new Compartment()), callback=useRef(onChange), syncing=useRef(false);
 const {language:locale}=useLanguage(); callback.current=onChange;
 const extensions=()=>[language(parser),EditorState.readOnly.of(disabled),EditorView.contentAttributes.of({'aria-label':label??t('请求正文'),spellcheck:'false'})];
 useEffect(()=>{
  const editor=new EditorView({parent:parent.current!,state:EditorState.create({doc:value,extensions:[config.current.of(extensions()),lineNumbers(),history(),foldGutter(),bracketMatching(),closeBrackets(),autocompletion(),syntaxHighlighting(defaultHighlightStyle),keymap.of([...completionKeymap,...closeBracketsKeymap,...defaultKeymap,...historyKeymap,...foldKeymap,indentWithTab]),EditorView.updateListener.of(update=>{if(update.docChanged&&!syncing.current)callback.current(update.state.doc.toString());}),EditorView.theme({'&':{height:'100%',minHeight:'220px',fontSize:'13px',backgroundColor:'var(--code-gutter, #fafafa)'},'.cm-scroller':{overflow:'auto',fontFamily:'Consolas, monospace'},'.cm-gutters':{backgroundColor:'var(--code-gutter, #fafafa)',color:'var(--code-muted, #888)'},'.cm-tooltip':{backgroundColor:'var(--code-gutter, #fafafa)',color:'inherit'}})]})});
  view.current=editor;return()=>{editor.destroy();view.current=undefined};
 },[]);
 useEffect(()=>{view.current?.dispatch({effects:config.current.reconfigure(extensions())})},[parser,disabled,locale,label]);
 useEffect(()=>{const editor=view.current;if(editor&&editor.state.doc.toString()!==value){syncing.current=true;try{editor.dispatch({changes:{from:0,to:editor.state.doc.length,insert:value}})}finally{syncing.current=false}}},[value]);
 return <div className="body-code-editor" ref={parent} style={{flex:1,minHeight:220,overflow:'hidden'}}/>;
}
