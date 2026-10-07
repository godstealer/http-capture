import { t } from './i18n';
import { useEffect, useRef } from 'react';
import { EditorState } from '@codemirror/state';
import { EditorView, lineNumbers, keymap, highlightActiveLineGutter } from '@codemirror/view';
import { foldGutter, foldKeymap, syntaxHighlighting, defaultHighlightStyle, HighlightStyle, bracketMatching } from '@codemirror/language';
import { defaultKeymap } from '@codemirror/commands';
import { html } from '@codemirror/lang-html';
import { javascript } from '@codemirror/lang-javascript';
import { json } from '@codemirror/lang-json';
import { css } from '@codemirror/lang-css';

const themedHighlight = HighlightStyle.define(defaultHighlightStyle.specs.map(spec => ({
  ...spec, ...(spec.color ? { color: `var(--syntax-${String(spec.color).slice(1)}, ${spec.color})` } : {}),
})));

export default function CodeView({ text, language, label = t("响应代码，可通过行号旁箭头折叠") }: { text: string; language: string; label?: string }) {
  const parent = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const grammar = language === 'html' ? html() : language === 'babel' ? javascript() : language === 'json' ? json() : language === 'css' ? css() : [];
    const editor = new EditorView({ parent: parent.current!, state: EditorState.create({ doc: text, extensions: [
      EditorState.readOnly.of(true), EditorView.editable.of(false), lineNumbers(), foldGutter(), bracketMatching(),
      highlightActiveLineGutter(), syntaxHighlighting(themedHighlight), keymap.of([...defaultKeymap, ...foldKeymap]), grammar,
      EditorView.theme({ '&': { height: '100%', fontSize: '13px' }, '.cm-scroller': { overflow: 'auto', fontFamily: 'Consolas, monospace', lineHeight: '1.7' },
        '.cm-gutters': { backgroundColor: 'var(--code-gutter, #fafafa)', color: 'var(--code-muted, #9b9b9b)', borderRight: '1px solid var(--code-border, #ececec)' },
        '.cm-activeLineGutter': { backgroundColor: 'var(--code-active, #fff4df)' }, '.cm-content': { padding: '10px 0' },
        '.cm-line': { padding: '0 12px' }, '&.cm-focused': { outline: 'none' } }),
    ] }) });
    return () => editor.destroy();
  }, [text, language]);
  return <div className="response-code-editor" ref={parent} aria-label={label} />;
}
