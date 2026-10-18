import {useLayoutEffect, useRef, useState} from 'react';
import {Compartment, EditorState, Transaction} from '@codemirror/state';
import {EditorView, lineNumbers, keymap} from '@codemirror/view';
import {history, historyKeymap, defaultKeymap, indentWithTab, isolateHistory} from '@codemirror/commands';
import {StreamLanguage, foldGutter, foldKeymap, syntaxHighlighting, defaultHighlightStyle, HighlightStyle, bracketMatching, indentOnInput} from '@codemirror/language';
import {autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap} from '@codemirror/autocomplete';
import {json} from '@codemirror/lang-json';
import {javascript} from '@codemirror/lang-javascript';
import {html} from '@codemirror/lang-html';
import {css} from '@codemirror/lang-css';
import {t, useLanguage} from './i18n';
import {autoFormatPasteEnabled, setAutoFormatPaste, useAutoFormatPaste} from './bodyEditorPreferences';
import {createBodyFormatController, type BodyFormatStatus, type BodyFormatTask} from './bodyFormatController';
import './body-code-editor.css';

const protoLanguage=StreamLanguage.define({startState:()=>({comment:false}),token(stream,state){if(state.comment){if(stream.skipTo('*/')){stream.match('*/');state.comment=false}else stream.skipToEnd();return 'comment'}if(stream.eatSpace())return null;if(stream.match('//')){stream.skipToEnd();return 'comment'}if(stream.match('/*')){state.comment=true;return 'comment'}if(stream.match(/"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'/))return 'string';if(stream.match(/\b(?:syntax|package|import|message|enum|service|rpc|returns|stream|option|repeated|optional|required|oneof|map|reserved|extensions|extend)\b/))return 'keyword';if(stream.match(/\b(?:string|bytes|bool|double|float|s?fixed(?:32|64)|s?int(?:32|64)|uint(?:32|64))\b/))return 'typeName';if(stream.match(/-?\d+(?:\.\d+)?/))return 'number';stream.next();return null}});
const language = (parser:string) => parser==='proto'?protoLanguage:parser==='json'?json():parser==='babel'?javascript():parser==='html'?html():parser==='css'?css():[];
const supported = (parser: string) => ['json', 'html', 'babel', 'css'].includes(parser);
const highlight = HighlightStyle.define(defaultHighlightStyle.specs.map(spec => ({...spec,
  ...(spec.color ? {color: `var(--syntax-${String(spec.color).slice(1)}, ${spec.color})`} : {}),
})));

function startFormat(text: string, parser: string): BodyFormatTask {
  let cancel = () => {};
  const promise = new Promise<string>((resolve, reject) => {
    let worker: Worker | undefined, timer: ReturnType<typeof setTimeout> | undefined, settled = false;
    const finish = (action: () => void) => {
      if (settled) return;
      settled = true; clearTimeout(timer); worker?.terminate(); action();
    };
    cancel = () => finish(() => reject(new Error('cancelled')));
    try {
      worker = new Worker(new URL('./body-format.worker.ts', import.meta.url), {type: 'module'});
      worker.onmessage = event => finish(() => typeof event.data.text === 'string'
        ? resolve(event.data.text) : reject(new Error(event.data.error || t('格式化失败，原文已保留'))));
      worker.onerror = event => { event.preventDefault(); finish(() => reject(new Error(t('格式化失败，原文已保留')))); };
      timer = setTimeout(() => finish(() => reject(new Error(t('格式化超时，原文已保留')))), 8000);
      worker.postMessage({text, parser});
    } catch (error) { finish(() => reject(error)); }
  });
  return {promise, cancel: () => cancel()};
}

export default function BodyCodeEditor({value, parser, onChange, disabled=false, label, formatting=false}: {
  label?: string; value: string; parser: string; onChange: (text: string) => void; disabled?: boolean; formatting?: boolean;
}) {
  const parent = useRef<HTMLDivElement>(null), view = useRef<EditorView>();
  const config = useRef(new Compartment()), syncing = useRef(false), applyingFormat = useRef(false), revision = useRef(0);
  const published = useRef(value), incoming = useRef({value, revision: 0});
  if (incoming.current.value !== value) {
    incoming.current.value = value;
    // A parent echo of this editor's own change is not a different document.
    // An external replacement invalidates workers even before layout effects.
    if (published.current !== value) incoming.current.revision++;
  }
  const props = useRef({onChange, parser, disabled, formatting});
  props.current = {onChange, parser, disabled, formatting};
  const formatter = useRef<ReturnType<typeof createBodyFormatController>>();
  const [status, setStatus] = useState<BodyFormatStatus>({kind: 'idle'});
  const automatic = useAutoFormatPaste();
  const {language: locale} = useLanguage();
  const extensions = () => [language(parser), EditorState.readOnly.of(disabled), EditorView.editable.of(!disabled),
    EditorView.contentAttributes.of({'aria-label': label ?? t('请求正文'), spellcheck: 'false'})];
  useLayoutEffect(() => {
    const editor = new EditorView({parent: parent.current!, state: EditorState.create({doc: value, extensions: [
      config.current.of(extensions()), lineNumbers(), history(), foldGutter(), bracketMatching(), indentOnInput(),
      closeBrackets(), autocompletion(), syntaxHighlighting(highlight),
      keymap.of([{key: 'Shift-Alt-f', run: () => {
        if (!props.current.formatting || props.current.disabled || !supported(props.current.parser)) return false;
        void formatter.current?.format('manual'); return true;
      }}, ...completionKeymap, ...closeBracketsKeymap, ...defaultKeymap, ...historyKeymap, ...foldKeymap, indentWithTab]),
      EditorView.updateListener.of(update => {
        if (!update.docChanged) return;
        revision.current++;
        if (!applyingFormat.current) formatter.current?.invalidate();
        if (!syncing.current) {
          const text = update.state.doc.toString();
          published.current = text;
          props.current.onChange(text);
        }
        if (!syncing.current && update.transactions.some(transaction => transaction.isUserEvent('input.paste'))) {
          const pastedRevision = revision.current;
          queueMicrotask(() => {
            if (view.current === editor && revision.current === pastedRevision && props.current.formatting) void formatter.current?.format('paste');
          });
        }
      }),
      EditorView.theme({'&': {height: '100%', minHeight: '220px', fontSize: '13px', color: 'var(--field-text, #303b49)', backgroundColor: 'var(--field-bg, #fff)'},
        '.cm-scroller': {overflow: 'auto', fontFamily: 'Consolas, monospace'},
        '.cm-gutters': {backgroundColor: 'var(--code-gutter, #fafafa)', color: 'var(--code-muted, #888)'},
        '.cm-tooltip': {backgroundColor: 'var(--code-gutter, #fafafa)', color: 'inherit'}}),
    ]})});
    view.current = editor;
    const controller = createBodyFormatController({
      read: () => ({text: editor.state.doc.toString(), parser: props.current.parser, revision: `${revision.current}:${incoming.current.revision}`,
        readOnly: props.current.disabled || !props.current.formatting, autoFormatOnPaste: autoFormatPasteEnabled()}),
      start: startFormat,
      apply: text => {
        applyingFormat.current = true;
        try { editor.dispatch({changes: {from: 0, to: editor.state.doc.length, insert: text},
          annotations: [Transaction.userEvent.of('input.format'), isolateHistory.of('full')]}); }
        finally { applyingFormat.current = false; }
      },
      status: setStatus,
    });
    formatter.current = controller;
    return () => { controller.dispose(); formatter.current = undefined; view.current = undefined; editor.destroy(); };
  }, []);
  useLayoutEffect(() => { view.current?.dispatch({effects: config.current.reconfigure(extensions())}); }, [parser, disabled, locale, label]);
  useLayoutEffect(() => { formatter.current?.invalidate(); }, [parser, disabled, formatting, automatic]);
  useLayoutEffect(() => {
    const editor = view.current;
    if (editor && editor.state.doc.toString() !== value) {
      syncing.current = true;
      try { editor.dispatch({changes: {from: 0, to: editor.state.doc.length, insert: value}, annotations: Transaction.addToHistory.of(false)}); }
      finally { syncing.current = false; }
    }
    published.current = value;
  }, [value]);
  const canFormat = formatting && !disabled && supported(parser);
  return <div className="body-code-editor">
    {formatting && <div className="body-code-tools">
      <button type="button" disabled={!canFormat || status.kind === 'formatting'} onClick={() => void formatter.current?.format('manual')}>
        {status.kind === 'formatting' ? t('格式化中…') : t('格式化')}
      </button>
      <label><input type="checkbox" checked={automatic} disabled={!canFormat} onChange={event => {
        formatter.current?.invalidate(); setAutoFormatPaste(event.target.checked);
      }}/>{t('粘贴后自动格式化')}</label>
      <span>{t('Shift+Alt+F 格式化')}</span>
    </div>}
    <div className="body-code-content" ref={parent}/>
    {formatting && status.kind !== 'idle' && <div className={`body-code-status ${status.kind === 'error' || status.kind === 'too-large' ? 'error' : ''}`}
      role={status.kind === 'error' || status.kind === 'too-large' ? 'alert' : 'status'}>
      {status.kind === 'formatted' ? t('已格式化 · 可撤销') : status.kind === 'too-large' ? t('正文超过 1 MiB，保留原文，不自动格式化')
        : status.kind === 'error' ? `${t('格式化失败，原文已保留')}：${status.message}` : t('格式化中…')}
    </div>}
  </div>;
}
