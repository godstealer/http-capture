import { useRef, type InputHTMLAttributes, type KeyboardEvent } from 'react';

type Props = Omit<InputHTMLAttributes<HTMLInputElement>, 'value' | 'onChange'> & {
  value: string;
  onValueChange: (value: string) => void;
};

type History = { current: string; undo: string[]; redo: string[]; lastEdit: number; lastKind: string };

// Keep text history beside the field: rerendering a controlled input can clear
// WebView's native undo stack before the next key press.
export default function UndoableInput({ value, onValueChange, onKeyDown, ...props }: Props) {
  const history = useRef<History>({ current: value, undo: [], redo: [], lastEdit: 0, lastKind: '' });
  if (history.current.current !== value) history.current = { current: value, undo: [], redo: [], lastEdit: 0, lastKind: '' };

  function change(next: string, kind: string) {
    const state = history.current;
    if (next === state.current) return;
    const now = Date.now();
    const continuous = kind === 'insertText' && state.lastKind === kind && now - state.lastEdit < 750;
    if (!continuous) state.undo.push(state.current);
    if (state.undo.length > 100) state.undo.shift();
    state.redo = [];
    state.current = next;
    state.lastEdit = now;
    state.lastKind = kind;
    onValueChange(next);
  }

  function keyDown(event: KeyboardEvent<HTMLInputElement>) {
    onKeyDown?.(event);
    if (event.defaultPrevented || !(event.ctrlKey || event.metaKey) || event.altKey) return;
    const key = event.key.toLowerCase();
    const undo = key === 'z' && !event.shiftKey;
    const redo = (key === 'z' && event.shiftKey) || (key === 'y' && !event.shiftKey);
    if (!undo && !redo) return;
    event.preventDefault();
    const state = history.current;
    const source = undo ? state.undo : state.redo;
    if (!source.length) return;
    const target = undo ? state.redo : state.undo;
    target.push(state.current);
    state.current = source.pop()!;
    state.lastKind = '';
    onValueChange(state.current);
    const input = event.currentTarget;
    const position = state.current.length;
    requestAnimationFrame(() => { if (document.activeElement === input) input.setSelectionRange(position, position); });
  }

  return <input {...props} value={value} onChange={event => change(event.target.value, (event.nativeEvent as InputEvent).inputType ?? '')} onKeyDown={keyDown} />;
}
