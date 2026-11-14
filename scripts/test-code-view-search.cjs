const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const { EditorState, Transaction } = require('@codemirror/state');
const viewModule = require('@codemirror/view');
const searchModule = require('@codemirror/search');
const { deleteCharBackward, insertNewlineAndIndent } = require('@codemirror/commands');
// Load browser feature detection before installing the small DOM fixture.
for (const language of ['html', 'javascript', 'json', 'css']) require(`@codemirror/lang-${language}`);

// The actual CodeMirror state, keymap and search commands run below. Only the
// EditorView DOM constructor and React's lifecycle are replaced: no native
// executable, browser dependency or network is needed for these regressions.
class Element {
  constructor(tag = 'div', classes = '', attrs = {}) {
    this.tagName = tag.toUpperCase(); this.classes = new Set(classes.split(/\s+/).filter(Boolean));
    this.attrs = attrs; this.children = []; this.parentElement = null; this.visible = true;
  }
  append(child) { child.remove(); this.children.push(child); child.parentElement = this; return child; }
  remove() { if (this.parentElement) this.parentElement.children = this.parentElement.children.filter(child => child !== this); this.parentElement = null; }
  matches(selector) {
    selector = selector.trim();
    const separator = selector.lastIndexOf('>');
    if (separator >= 0) return this.matches(selector.slice(separator + 1)) && !!this.parentElement?.matches(selector.slice(0, separator));
    const space = selector.lastIndexOf(' ');
    if (space >= 0) {
      if (!this.matches(selector.slice(space + 1))) return false;
      for (let node = this.parentElement; node; node = node.parentElement) if (node.matches(selector.slice(0, space))) return true;
      return false;
    }
    const tag = /^[a-z]+/i.exec(selector);
    if (tag && this.tagName !== tag[0].toUpperCase()) return false;
    for (const match of selector.matchAll(/\.([\w-]+)/g)) if (!this.classes.has(match[1])) return false;
    for (const match of selector.matchAll(/\[([\w-]+)(?:=["']?([^\]"']+)["']?)?\]/g)) {
      if (!(match[1] in this.attrs) || match[2] !== undefined && this.attrs[match[1]] !== match[2]) return false;
    }
    return true;
  }
  closest(selector) {
    for (let node = this; node; node = node.parentElement) if (selector.split(',').some(part => node.matches(part))) return node;
    return null;
  }
  contains(other) { for (let node = other; node; node = node.parentElement) if (node === this) return true; return false; }
  querySelectorAll(selector) {
    return this.children.flatMap(child => [...(selector.split(',').some(part => child.matches(part)) ? [child] : []), ...child.querySelectorAll(selector)]);
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  getClientRects() {
    for (let node = this; node; node = node.parentElement) if (!node.visible) return [];
    return document.body.contains(this) ? [{}] : [];
  }
  get tabIndex() { return 'tabindex' in this.attrs ? Number(this.attrs.tabindex) : ['INPUT', 'TEXTAREA', 'BUTTON'].includes(this.tagName) ? 0 : -1; }
  focus() { this.focusCount = (this.focusCount ?? 0) + 1; document.activeElement = this; }
  // Intentionally do not focus here: this reproduces non-editable WebViews
  // where SearchPanel.mount's select() alone leaves keyboard focus behind.
  select() { this.selectCount = (this.selectCount ?? 0) + 1; }
}
global.Element = global.HTMLElement = Element;
global.document = { body: new Element('body'), activeElement: null };
const listeners = new Set();
global.window = {
  addEventListener(name, callback) { assert.equal(name, 'keydown'); listeners.add(callback); },
  removeEventListener(name, callback) { assert.equal(name, 'keydown'); listeners.delete(callback); },
};
const add = (parent, classes = '', tag = 'div', attrs = {}) => parent.append(new Element(tag, classes, attrs));
const opened = [], created = [];
class EditorView {
  static editable = viewModule.EditorView.editable;
  static contentAttributes = viewModule.EditorView.contentAttributes;
  static theme = viewModule.EditorView.theme;
  constructor({ parent, state }) {
    this.state = state; this.parent = parent; this.root = document;
    this.dom = add(parent, 'cm-editor');
    const attributes = Object.assign({ contenteditable: String(state.facet(EditorView.editable)) },
      ...state.facet(EditorView.contentAttributes).map(value => typeof value === 'function' ? value(this) : value));
    this.contentDOM = add(this.dom, 'cm-content', 'div', attributes);
    this.dispatch = this.dispatch.bind(this); created.push(this);
  }
  dispatch(spec) {
    assert(!this.destroyed); this.state = (spec instanceof Transaction ? spec : this.state.update(spec)).state;
    if (searchModule.searchPanelOpen(this.state)) {
      if (!this.searchField) {
        this.searchPanel = add(this.dom, 'cm-panel cm-search');
        this.searchField = add(this.searchPanel, 'cm-textfield', 'input', { 'main-field': 'true' });
        this.searchField.select(); // Model the real SearchPanel.mount lifecycle.
      }
    } else if (this.searchField) {
      this.searchPanel.remove(); this.searchPanel = this.searchField = null;
    }
  }
  plugin() { return null; } // No real view plugins/panels are mounted in this fixture.
  focus() { this.contentDOM.focus(); }
  destroy() { this.destroyed = true; this.dom.remove(); }
}
const compiled = buildSync({
  entryPoints: ['apps/frontend/src/CodeView.tsx'], bundle: true, platform: 'node',
  format: 'cjs', write: false, jsx: 'automatic', packages: 'external', external: ['./i18n'],
}).outputFiles[0].text;
let active;
const react = {
  useRef(value) { const cell = active.cursor++; return active.hooks[cell] ??= { current: value }; },
  useEffect(effect, deps) {
    const h = active, index = h.cursor++, previous = h.hooks[index];
    if (!previous || deps.some((value, i) => !Object.is(value, previous[i]))) {
      h.hooks[index] = deps; h.effects.push(() => { h.cleanups[index]?.(); h.cleanups[index] = effect(); });
    }
  },
};
const jsx = (type, props) => {
  const element = props.ref?.current ?? new Element(type, props.className, Object.fromEntries(Object.entries(props).filter(([key]) => key.startsWith('aria-'))));
  if (props.ref) props.ref.current = element;
  return { type, props, element };
};
const loaded = { exports: {} };
new Function('require', 'module', 'exports', compiled)(name => {
  if (name === 'react') return react;
  if (name === 'react/jsx-runtime') return { jsx, jsxs: jsx };
  if (name === './i18n') return { t: value => value, useLanguage: () => ({ language: 'zh-CN' }) };
  if (name === '@codemirror/view') return { ...viewModule, EditorView };
  if (name === '@codemirror/search') return { ...searchModule, openSearchPanel(view) { opened.push(view); return searchModule.openSearchPanel(view); } };
  return require(name);
}, loaded, loaded.exports);
const CodeView = loaded.exports.default;
const mounted = [];
function mount(parent, label, text = '{"first":"needle","second":"needle"}') {
  const h = { cursor: 0, hooks: [], effects: [], cleanups: [], props: { text, language: 'json', label } };
  h.render = () => {
    active = h; h.cursor = 0; h.tree = CodeView(h.props);
    if (!h.tree.element.parentElement) parent.append(h.tree.element);
    while (h.effects.length) h.effects.shift()();
    h.view = created.findLast(view => view.parent === h.tree.element && !view.destroyed);
  };
  h.update = props => { h.props = { ...h.props, ...props }; h.render(); };
  h.unmount = () => { h.cleanups.forEach(cleanup => cleanup?.()); h.tree.element.remove(); };
  h.render(); mounted.push(h); return h;
}
function key(target, overrides = {}) {
  return { target, key: 'f', keyCode: 70, ctrlKey: true, metaKey: false, altKey: false, shiftKey: false, defaultPrevented: false,
    preventDefault() { this.defaultPrevented = true; }, stopPropagation() {}, ...overrides };
}
function emit(event) { for (const listener of [...listeners]) listener(event); return event; }
function assertSearchFocus(view, previous) {
  const field = view.searchField;
  assert(field, 'opening search mounts its main-field input');
  assert.equal(document.activeElement, field, 'Ctrl+F must move keyboard focus into the response search field');
  assert.equal(field.focusCount, (previous?.field === field ? previous.focusCount : 0) + 1, 'the response helper explicitly focuses the field once');
  assert.equal(field.selectCount, (previous?.field === field ? previous.selectCount : 1) + 1, 'the response helper explicitly selects the query after focusing it');
}
function searchFocusBefore(view) {
  return { field: view.searchField, focusCount: view.searchField?.focusCount ?? 0, selectCount: view.searchField?.selectCount ?? 0 };
}
function routes(target, expected, overrides) {
  const before = opened.length, beforeFocus = searchFocusBefore(expected.view), event = emit(key(target, overrides));
  assert.equal(opened.length, before + 1, 'one owning editor handles the shortcut');
  assert.equal(opened.at(-1), expected.view); assert(event.defaultPrevented);
  assert(searchModule.searchPanelOpen(expected.view.state));
  assertSearchFocus(expected.view, beforeFocus);
}
function ignores(target, overrides) {
  const before = opened.length, event = emit(key(target, overrides));
  assert.equal(opened.length, before, 'unrelated/handled shortcut must not open a background response search');
  if (!overrides?.defaultPrevented) assert(!event.defaultPrevented);
}
try {
  const capture = add(document.body, 'capture-inspector'), split = add(capture, 'inspector-split');
  const requestPane = add(split, 'split-content'), responsePane = add(split, 'split-content');
  const requestToolbar = add(add(requestPane, '', 'section'), 'inspector-tabs');
  const responseToolbar = add(add(responsePane, '', 'section'), 'inspector-tabs');
  const requestBody = add(requestPane, 'formatted-response'), responseBody = add(responsePane, 'formatted-response');
  const request = mount(requestBody, '请求正文'), response = mount(responseBody, '响应正文');
  const hidden = mount(responseBody, '隐藏的响应'); hidden.tree.element.visible = false;
  const attrs = Object.assign({}, ...response.view.state.facet(viewModule.EditorView.contentAttributes));
  assert.equal(response.view.state.readOnly, true);
  assert.equal(response.view.state.facet(viewModule.EditorView.editable), false);
  assert.equal(attrs.tabindex, '0'); assert.equal(attrs.role, 'textbox');
  assert.equal(attrs['aria-readonly'], 'true'); assert.equal(attrs['aria-label'], '响应正文');
  assert.equal(response.view.contentDOM.tabIndex, 0, 'non-editable response content must be keyboard focusable');
  assert.equal(response.tree.props['aria-keyshortcuts'], 'Control+f Meta+f');
  assert(response.view.state.facet(viewModule.keymap).flat().some(binding => binding.key === 'Mod-f'), 'the real editor has the standard search binding');

  // Exercise the real CodeMirror shortcut and search state from focused read-only content.
  response.view.focus(); assert.equal(document.activeElement, response.view.contentDOM);
  const beforeNative = opened.length, beforeNativeFocus = searchFocusBefore(response.view);
  let event = key(document.activeElement);
  let handled = viewModule.runScopeHandlers(response.view, event, 'editor');
  if (!handled) { event = key(document.activeElement, { ctrlKey: false, metaKey: true }); handled = viewModule.runScopeHandlers(response.view, event, 'editor'); }
  assert(handled, 'CodeMirror handles the platform Mod-f binding'); event.preventDefault(); emit(event);
  assert(searchModule.searchPanelOpen(response.view.state));
  assert.equal(opened.length, beforeNative + 1, 'the editor uses the focused-search override once and the global fallback does not open it again');
  assertSearchFocus(response.view, beforeNativeFocus);
  response.view.dispatch({ effects: searchModule.setSearchQuery.of(new searchModule.SearchQuery({ search: 'needle' })) });
  assert(searchModule.findNext(response.view));
  assert.equal(response.view.state.selection.main.from, response.props.text.indexOf('needle'));
  assert(searchModule.findNext(response.view));
  assert.equal(response.view.state.selection.main.from, response.props.text.lastIndexOf('needle'));
  const unchanged = response.view.state.doc.toString();
  assert.equal(deleteCharBackward(response.view), false); assert.equal(insertNewlineAndIndent(response.view), false);
  assert.equal(response.view.state.doc.toString(), unchanged, 'focusing/searching a read-only response must not enable editing');

  routes(requestBody, request); routes(responseBody, response);
  routes(requestToolbar, request); routes(responseToolbar, response);
  routes(responseToolbar, response, { ctrlKey: false, metaKey: true });
  searchModule.closeSearchPanel(response.view); assert(!searchModule.searchPanelOpen(response.view.state));
  routes(responseToolbar, response); // Closing and pressing Ctrl+F again reopens it.
  response.tree.element.visible = false; ignores(responseToolbar); response.tree.element.visible = true;

  ignores(add(responseToolbar, '', 'input')); ignores(add(requestToolbar, '', 'textarea'));
  ignores(add(responseBody, '', 'div', { contenteditable: 'true' }));
  const foreignEditor = add(responseBody, 'cm-editor'); ignores(add(foreignEditor, 'cm-content'));
  ignores(response.view.contentDOM); // Editor's own keymap, not the region fallback, owns it.
  ignores(responseToolbar, { shiftKey: true }); ignores(responseToolbar, { altKey: true });
  ignores(responseToolbar, { ctrlKey: false }); ignores(responseToolbar, { key: 'g' });
  ignores(responseToolbar, { defaultPrevented: true }); ignores({}); ignores(document.body);

  const detail = add(document.body, 'detail-panel'), detailToolbar = add(detail, 'tabs');
  const replayRequest = add(detail, 'replay-request'), replayResponse = add(detail, 'replay-response');
  const replayRequestBody = add(replayRequest, 'formatted-response'), replayResponseBody = add(replayResponse, 'formatted-response');
  const detailRequest = mount(replayRequestBody, '重放请求'), detailResponse = mount(replayResponseBody, '重放响应');
  const lastHidden = mount(replayResponseBody, '不显示的最后编辑器'); lastHidden.tree.element.visible = false;
  routes(detailToolbar, detailResponse); // Region fallback picks the last visible editor.
  routes(add(replayResponse, 'response-tabs'), detailResponse); routes(replayRequestBody, detailRequest);
  const unrelatedDialog = add(detail, '', 'div', { role: 'dialog' }); ignores(add(unrelatedDialog, '', 'button'));
  const ownDialog = add(detail, '', 'div', { role: 'dialog' }), dialogBody = add(ownDialog, 'formatted-response');
  const dialogView = mount(dialogBody, '对话框内响应'); routes(add(dialogBody, 'format-toolbar'), dialogView);

  const beforeLabel = response.view, listenerCount = listeners.size;
  response.update({ label: '当前响应正文' }); assert(beforeLabel.destroyed);
  assert.equal(response.view.contentDOM.attrs['aria-label'], '当前响应正文');
  assert.equal(listeners.size, listenerCount, 'rerendering the label must not add another global handler');
  routes(responseToolbar, response);
  const beforeCommand = searchFocusBefore(response.view);
  response.update({ command: { type: 'find', id: 1 } }); assert.equal(opened.at(-1), response.view);
  assertSearchFocus(response.view, beforeCommand);
  const oldSearchField = response.view.searchField;
  searchModule.closeSearchPanel(response.view);
  response.update({ command: { type: 'find', id: 2 } }); assert(searchModule.searchPanelOpen(response.view.state));
  assert.notEqual(response.view.searchField, oldSearchField, 'reopening creates a new search field after closing');
  assertSearchFocus(response.view);
} finally {
  mounted.forEach(instance => instance.unmount());
}
assert.equal(listeners.size, 0, 'unmount removes every global keydown listener');
assert(created.every(view => view.destroyed), 'unmount and replacement destroy all editor instances');
console.log('Response search: real read-only CodeMirror state/keymap/find-next, explicit query focus/selection and focus attributes, pane routing, visible fallbacks, modifiers, native/foreign editor/dialog isolation, repeated search and listener cleanup passed.');
