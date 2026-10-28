import { t, useLanguage } from './i18n';
import { useEffect, useRef } from 'react';
import { EditorState } from '@codemirror/state';
import { EditorView, lineNumbers, keymap, highlightActiveLineGutter } from '@codemirror/view';
import { foldAll, foldGutter, foldKeymap, foldService, unfoldAll, syntaxHighlighting, defaultHighlightStyle, HighlightStyle, bracketMatching } from '@codemirror/language';
import { defaultKeymap } from '@codemirror/commands';
import { openSearchPanel, search, searchKeymap } from '@codemirror/search';
import { html } from '@codemirror/lang-html';
import { javascript } from '@codemirror/lang-javascript';
import { json } from '@codemirror/lang-json';
import { css } from '@codemirror/lang-css';
import { jsonFoldRanges } from './jsonFoldRanges';

const themedHighlight = HighlightStyle.define(defaultHighlightStyle.specs.map(spec => ({
  ...spec, ...(spec.color ? { color: `var(--syntax-${String(spec.color).slice(1)}, ${spec.color})` } : {}),
})));

export type CodeViewCommand = { type: 'find' | 'fold' | 'unfold'; id: number };

export default function CodeView({ text, language, label = t("响应代码，可通过行号旁箭头折叠"), command }: { text: string; language: string; label?: string; command?: CodeViewCommand | null }) {
  const parent = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const { language: locale } = useLanguage();
  useEffect(() => {
    const grammar = language === 'html' ? html() : language === 'babel' ? javascript() : language === 'json' ? json() : language === 'css' ? css() : [];
    // EditorState normalizes CRLF/CR to LF when constructing its document.
    const jsonRanges = language === 'json' ? jsonFoldRanges(text.replace(/\r\n?|\n/g, '\n')) : null;
    const editor = new EditorView({ parent: parent.current!, state: EditorState.create({ doc: text, extensions: [
      EditorState.readOnly.of(true), EditorView.editable.of(false), lineNumbers(), foldGutter(), bracketMatching(),
      ...(jsonRanges ? [foldService.of((_state, from) => jsonRanges.get(from) ?? null)] : []),
      highlightActiveLineGutter(), syntaxHighlighting(themedHighlight), search(),
      EditorState.phrases.of({ Find: t('查找'), next: t('下一个'), previous: t('上一个'), all: t('全部'),
        'match case': t('区分大小写'), regexp: t('正则表达式'), 'by word': t('全词匹配'), close: t('关闭'),
        'Fold line': t('折叠此节点'), 'Unfold line': t('展开此节点') }),
      keymap.of([...searchKeymap, ...defaultKeymap, ...foldKeymap]), grammar,
      EditorView.theme({ '&': { height: '100%', fontSize: '13px' }, '.cm-scroller': { overflow: 'auto', fontFamily: 'Consolas, monospace', lineHeight: '1.7' },
        '.cm-gutters': { backgroundColor: 'var(--code-gutter, #fafafa)', color: 'var(--code-muted, #9b9b9b)', borderRight: '1px solid var(--code-border, #ececec)' },
        '.cm-activeLineGutter': { backgroundColor: 'var(--code-active, #fff4df)' }, '.cm-content': { padding: '10px 0' },
        '.cm-line': { padding: '0 12px' }, '&.cm-focused': { outline: 'none' },
        '.cm-panel.cm-search': { backgroundColor: 'var(--ui-surface, var(--field-bg, #fff))', color: 'var(--field-text, #303b49)', borderBottom: '1px solid var(--ui-border, #dce3ea)' },
        '.cm-panel.cm-search .cm-textfield': { backgroundColor: 'var(--field-bg, #fff)', color: 'var(--field-text, #303b49)' },
        '.cm-foldGutter': { minWidth: '20px' }, '.cm-foldGutter span': { display: 'inline-block', minWidth: '16px', textAlign: 'center' } }),
    ] }) });
    view.current = editor;
    return () => { view.current = null; editor.destroy(); };
  }, [text, language, locale]);
  useEffect(() => {
    const editor = view.current;
    if (!editor || !command) return;
    if (command.type === 'find') openSearchPanel(editor);
    else if (command.type === 'fold') foldAll(editor);
    else unfoldAll(editor);
  }, [command]);
  return <div className="response-code-editor" ref={parent} aria-label={label} />;
}
