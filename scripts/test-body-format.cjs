const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const loaded = { exports: {} };
new Function('module', 'exports', buildSync({
  entryPoints: ['apps/frontend/src/bodyFormatController.ts'],
  bundle: true, platform: 'node', format: 'cjs', write: false,
}).outputFiles[0].text)(loaded, loaded.exports);
const { createBodyFormatController, BODY_FORMAT_LIMIT } = loaded.exports;
// Unsettled Promises alone do not keep Node alive. Make a cancellation regression
// fail explicitly instead of silently exiting before the assertions complete.
const watchdog = setTimeout(() => {
  console.error('Body formatting controller tests did not settle');
  process.exit(1);
}, 10000);

function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

function harness(initial = {}, hooks = {}) {
  const state = {
    snapshot: { text: '{"n":1}', parser: 'json', revision: 1, readOnly: false, autoFormatOnPaste: true, ...initial },
    jobs: [], applied: [], statuses: [],
  };
  state.controller = createBodyFormatController({
    read: () => state.snapshot,
    start: (text, parser) => {
      if (hooks.start) return hooks.start(state, text, parser);
      const pending = deferred();
      const job = { ...pending, text, parser, cancellations: 0 };
      job.cancel = () => { job.cancellations++; };
      state.jobs.push(job);
      return job;
    },
    apply: text => {
      assert.notEqual(state.statuses.at(-1)?.kind, 'formatting', 'busy must end before the edit is applied');
      state.applied.push(text);
      hooks.apply?.(state, text);
    },
    status: status => { state.statuses.push(status); hooks.status?.(state, status); },
  });
  return state;
}
const tick = () => new Promise(resolve => setImmediate(resolve));

(async () => {
  // Paste remains exactly as entered unless the shared opt-in is enabled.
  const off = harness({ autoFormatOnPaste: false });
  await off.controller.format('paste');
  assert.equal(off.jobs.length, 0);
  assert.equal(off.snapshot.text, '{"n":1}');
  const manual = off.controller.format('manual');
  assert.equal(off.jobs.length, 1);
  off.jobs[0].resolve('{\n  "n": 1\n}');
  await manual;
  assert.deepEqual(off.applied, ['{\n  "n": 1\n}']);
  assert.deepEqual(off.statuses.map(status => status.kind), ['idle', 'formatting', 'formatted']);

  // Parser failures preserve the user's pasted source and can be retried.
  const invalid = harness({ text: '{"unfinished":' });
  const failed = invalid.controller.format('paste');
  invalid.jobs[0].reject(new SyntaxError('Unexpected end of JSON input'));
  await failed;
  assert.deepEqual(invalid.applied, []);
  assert.equal(invalid.snapshot.text, '{"unfinished":');
  assert.deepEqual(invalid.statuses.at(-1), { kind: 'error', message: 'Unexpected end of JSON input' });
  const retry = invalid.controller.format('manual');
  invalid.jobs[1].resolve('{}');
  await retry;
  assert.deepEqual(invalid.applied, ['{}']);

  // The caller may miss an invalidate notification; the final snapshot check
  // must still protect text, parser, preferences and editability independently.
  for (const changed of [
    { text: '{"new":true}' },
    { revision: 2 }, // Same text with a distinct document revision (ABA).
    { parser: 'html' },
    { readOnly: true },
    { autoFormatOnPaste: false },
  ]) {
    const stale = harness();
    const formatting = stale.controller.format('paste');
    stale.snapshot = { ...stale.snapshot, ...changed };
    stale.jobs[0].resolve('old formatted result');
    await formatting;
    assert.deepEqual(stale.applied, [], JSON.stringify(changed));
    assert.equal(stale.statuses.at(-1).kind, 'idle');
  }
  const manualSetting = harness({ autoFormatOnPaste: false });
  const manualSettingRun = manualSetting.controller.format('manual');
  manualSetting.snapshot.autoFormatOnPaste = true;
  manualSetting.jobs[0].resolve('stale manual result');
  await manualSettingRun;
  assert.deepEqual(manualSetting.applied, []);

  // Capture fields by value even when read() returns a reused mutable object.
  const mutable = harness();
  const mutableRun = mutable.controller.format('manual');
  mutable.snapshot.text = 'edited in place';
  mutable.jobs[0].resolve('stale');
  await mutableRun;
  assert.deepEqual(mutable.applied, []);

  // A second paste cancels the old worker and only its own result can win,
  // even if the old worker ignores cancellation and finishes last.
  const latest = harness();
  const older = latest.controller.format('paste');
  latest.snapshot = { ...latest.snapshot, text: '{"n":2}', revision: 2 };
  const newer = latest.controller.format('paste');
  assert.equal(latest.jobs[0].cancellations, 1);
  await older; // Cancellation settles format() without waiting for the worker.
  latest.jobs[1].resolve('{ "n": 2 }');
  await newer;
  latest.jobs[0].resolve('{ "n": 1 }');
  await tick();
  assert.deepEqual(latest.applied, ['{ "n": 2 }']);
  assert.equal(latest.statuses.at(-1).kind, 'formatted');
  assert.deepEqual(latest.jobs.map(job => [job.text, job.parser]), [['{"n":1}', 'json'], ['{"n":2}', 'json']]);

  // Explicit invalidation covers settings/document ABA and leaves no late
  // success or failure status behind when a cancelled task eventually settles.
  const cancelled = harness();
  const pending = cancelled.controller.format('manual');
  cancelled.controller.invalidate();
  assert.equal(cancelled.jobs[0].cancellations, 1);
  await pending;
  const afterInvalidation = cancelled.statuses.length;
  cancelled.jobs[0].reject(new Error('cancelled worker later rejected'));
  await tick();
  assert.equal(cancelled.statuses.length, afterInvalidation);
  assert.equal(cancelled.statuses.at(-1).kind, 'idle');
  assert.deepEqual(cancelled.applied, []);

  // An ineligible new operation also invalidates a previously running job.
  const disabled = harness();
  const beforeDisabled = disabled.controller.format('manual');
  disabled.snapshot.autoFormatOnPaste = false;
  await disabled.controller.format('paste');
  await beforeDisabled;
  assert.equal(disabled.jobs.length, 1);
  assert.equal(disabled.jobs[0].cancellations, 1);
  disabled.jobs[0].resolve('stale');
  await tick();
  assert.deepEqual(disabled.applied, []);

  const disposed = harness();
  const beforeDispose = disposed.controller.format('paste');
  disposed.controller.dispose();
  disposed.controller.dispose();
  await beforeDispose;
  const callbacksAtDispose = disposed.statuses.length;
  assert.equal(disposed.jobs[0].cancellations, 1);
  disposed.jobs[0].reject(new Error('late unmount rejection'));
  await disposed.controller.format('manual');
  disposed.controller.invalidate();
  await tick();
  assert.equal(disposed.statuses.length, callbacksAtDispose);
  assert.equal(disposed.jobs.length, 1);
  assert.deepEqual(disposed.applied, []);

  // Timeouts are task failures, not an instruction to replace pasted text.
  const timedOut = harness();
  const timeoutRun = timedOut.controller.format('paste');
  const timeout = new Error('Formatting timed out');
  timeout.name = 'TimeoutError';
  timedOut.jobs[0].reject(timeout);
  await timeoutRun;
  assert.equal(timedOut.statuses.at(-1).kind, 'error');
  assert.equal(timedOut.statuses.at(-1).message, 'Formatting timed out');
  assert.deepEqual(timedOut.applied, []);

  const throwing = harness({}, { start: () => { throw new Error('Worker unavailable'); } });
  await throwing.controller.format('manual');
  assert.deepEqual(throwing.statuses.at(-1), { kind: 'error', message: 'Worker unavailable' });

  const huge = harness({ text: 'x'.repeat(BODY_FORMAT_LIMIT + 1) });
  await huge.controller.format('manual');
  assert.equal(huge.jobs.length, 0);
  assert.equal(huge.statuses.at(-1).kind, 'too-large');
  const unicodeHuge = harness({ text: '中'.repeat(Math.ceil(BODY_FORMAT_LIMIT / 3)) });
  await unicodeHuge.controller.format('paste');
  assert.equal(unicodeHuge.jobs.length, 0, 'limit counts UTF-8 bytes, not only code units');
  assert.equal(unicodeHuge.statuses.at(-1).kind, 'too-large');
  const exact = harness({ text: 'x'.repeat(BODY_FORMAT_LIMIT) });
  const exactRun = exact.controller.format('manual');
  assert.equal(exact.jobs.length, 1);
  exact.jobs[0].resolve(exact.snapshot.text);
  await exactRun;
  assert.deepEqual(exact.applied, [], 'unchanged formatting must not create an undo edit');
  assert.equal(exact.statuses.at(-1).kind, 'formatted');
  for (const snapshot of [
    { text: '' }, { text: ' \r\n\t' }, { parser: 'text' }, { parser: 'base64' },
    { parser: 'javascript' }, { readOnly: true },
  ]) {
    const skipped = harness(snapshot);
    await skipped.controller.format('manual');
    await skipped.controller.format('paste');
    assert.equal(skipped.jobs.length, 0, JSON.stringify(snapshot));
    assert.equal(skipped.statuses.at(-1).kind, 'idle');
    assert.deepEqual(skipped.applied, []);
  }
  for (const parser of ['json', 'html', 'babel', 'css']) {
    const accepted = harness({ parser });
    const run = accepted.controller.format('manual');
    assert.equal(accepted.jobs[0].parser, parser);
    accepted.jobs[0].resolve('formatted');
    await run;
    assert.deepEqual(accepted.applied, ['formatted']);
  }

  // Editor callbacks may unmount or invalidate synchronously. Neither case
  // permits a worker start/apply after the callback invalidated the operation.
  const statusInvalidation = harness({}, {
    status: (state, status) => { if (status.kind === 'formatting') state.controller.invalidate(); },
  });
  await statusInvalidation.controller.format('manual');
  assert.equal(statusInvalidation.jobs.length, 0);
  const unmountOnSuccess = harness({}, {
    status: (state, status) => { if (status.kind === 'formatted') state.controller.dispose(); },
  });
  const beforeUnmount = unmountOnSuccess.controller.format('manual');
  unmountOnSuccess.jobs[0].resolve('formatted');
  await beforeUnmount;
  assert.deepEqual(unmountOnSuccess.applied, []);

  const startReentry = harness({}, {
    start: state => {
      const job = deferred();
      job.cancellations = 0;
      job.cancel = () => { job.cancellations++; throw new Error('worker cancellation failed'); };
      state.jobs.push(job);
      state.controller.dispose();
      return job;
    },
  });
  await startReentry.controller.format('manual');
  assert.equal(startReentry.jobs[0].cancellations, 1);
  startReentry.jobs[0].reject(new Error('late start failure'));
  await tick();
  assert.deepEqual(startReentry.applied, []);

  await editorIntegration();
  console.log('Body formatting: asynchronous ordering, cancellation, document replacement, separate paste/format undo and error recovery passed');
})().then(() => clearTimeout(watchdog), error => {
  clearTimeout(watchdog);
  console.error(error);
  process.exitCode = 1;
});

async function editorIntegration() {
  const { EditorState, Transaction, EditorSelection } = require('@codemirror/state');
  const viewModule = require('@codemirror/view');
  const { undo, redo } = require('@codemirror/commands');
  const code = buildSync({
    entryPoints: ['apps/frontend/src/BodyCodeEditor.tsx'], bundle: true,
    platform: 'node', format: 'cjs', jsx: 'automatic', write: false,
    loader: { '.css': 'empty' },
    define: { 'import.meta.url': '"http://editor.test/BodyCodeEditor.tsx"' },
    external: ['react', 'react/jsx-runtime', '@codemirror/*', './i18n', './bodyEditorPreferences'],
  }).outputFiles[0].text;
  const originalWorker = global.Worker;
  const workers = [];
  class Worker {
    constructor() { workers.push(this); }
    postMessage(message) { this.message = message; }
    terminate() { this.terminated = true; }
    complete(text) { this.onmessage({ data: { text } }); }
  }
  global.Worker = Worker;
  const automatic = { enabled: true };
  function mount(initial = '') {
    let props, view, tree, cursor = 0, dirty = true, mounted = true;
    const hooks = [], layouts = [], effects = [], changes = [];
    function different(previous, next) { return !previous || !next || next.some((value, index) => !Object.is(value, previous[index])); }
    function effect(queue, callback, dependencies) {
      const cell = hooks[cursor++] ??= {};
      if (different(cell.dependencies, dependencies)) {
        cell.dependencies = dependencies;
        queue.push(() => { cell.cleanup?.(); cell.cleanup = callback(); });
      }
    }
    const react = {
      ...require('react'),
      useRef(value) { return hooks[cursor++] ??= { current: value }; },
      useState(initialValue) {
        const cell = hooks[cursor++] ??= { value: typeof initialValue === 'function' ? initialValue() : initialValue };
        return [cell.value, value => {
          assert.ok(mounted, 'disposed formatter must not update React state');
          const next = typeof value === 'function' ? value(cell.value) : value;
          if (!Object.is(cell.value, next)) { cell.value = next; dirty = true; }
        }];
      },
      useLayoutEffect(callback, dependencies) { effect(layouts, callback, dependencies); },
      useEffect(callback, dependencies) { effect(effects, callback, dependencies); },
    };
    class EditorView {
      static updateListener = viewModule.EditorView.updateListener;
      static editable = viewModule.EditorView.editable;
      static contentAttributes = viewModule.EditorView.contentAttributes;
      static theme = viewModule.EditorView.theme;
      constructor(options) { this.state = options.state; view = this; this.dispatch = this.dispatch.bind(this); }
      dispatch(spec) {
        assert.ok(!this.destroyed);
        const transaction = spec instanceof Transaction ? spec : this.state.update(spec);
        this.state = transaction.state;
        const update = { state: this.state, transactions: [transaction], docChanged: transaction.docChanged };
        for (const listener of this.state.facet(EditorView.updateListener)) listener(update);
      }
      destroy() { this.destroyed = true; }
    }
    const module = { exports: {} };
    new Function('require', 'module', 'exports', code)(name => {
      if (name === 'react') return react;
      if (name === '@codemirror/view') return { ...viewModule, EditorView };
      if (name === './i18n') return { t: value => value, useLanguage: () => ({ language: 'zh-CN' }) };
      if (name === './bodyEditorPreferences') return {
        autoFormatPasteEnabled: () => automatic.enabled, useAutoFormatPaste: () => automatic.enabled,
        setAutoFormatPaste: value => { automatic.enabled = value; },
      };
      return require(name);
    }, module, module.exports);
    props = { value: initial, parser: 'json', formatting: true, onChange: text => {
      changes.push(text); props = { ...props, value: text }; dirty = true;
    } };
    function render(commit = true) {
      if (dirty) { dirty = false; cursor = 0; tree = module.exports.default(props); }
      if (commit) {
        while (layouts.length) layouts.shift()();
        while (effects.length) effects.shift()();
        if (dirty) render();
      }
    }
    function nodes(node) {
      if (Array.isArray(node)) return node.flatMap(nodes);
      if (!node || typeof node !== 'object') return [];
      return [node, ...nodes(node.props?.children)];
    }
    render();
    return {
      get view() { return view; }, changes,
      render,
      update(update, commit = true) { props = { ...props, ...update }; dirty = true; render(commit); },
      format() { render(); nodes(tree).find(node => node.type === 'button').props.onClick(); },
      unmount() { hooks.forEach(cell => cell?.cleanup?.()); mounted = false; },
    };
  }
  try {
    const ui = mount();
    const pasted = '{"n":1}', formatted = '{\n  "n": 1\n}\n';
    ui.view.dispatch({ changes: { from: 0, insert: pasted }, annotations: Transaction.userEvent.of('input.paste') });
    ui.render(); await Promise.resolve();
    assert.equal(workers.at(-1).message.text, pasted);
    workers.at(-1).complete(formatted); await tick(); ui.render();
    assert.equal(ui.view.state.doc.toString(), formatted);
    assert.equal(undo(ui.view), true); ui.render();
    assert.equal(ui.view.state.doc.toString(), pasted, 'first Undo removes formatting only');
    assert.equal(undo(ui.view), true); ui.render();
    assert.equal(ui.view.state.doc.toString(), '', 'second Undo removes the paste');
    assert.equal(redo(ui.view), true); ui.render();
    assert.equal(ui.view.state.doc.toString(), pasted);
    assert.equal(redo(ui.view), true); ui.render();
    assert.equal(ui.view.state.doc.toString(), formatted);
    ui.view.dispatch({ selection: EditorSelection.cursor(5) });
    ui.update({ value: formatted });
    assert.equal(ui.view.state.selection.main.head, 5, 'the parent value echo must not replace the document or move its cursor');

    ui.format(); const replacing = workers.at(-1);
    const count = ui.changes.length;
    // Simulate a yielded React render: incoming props are visible before its
    // layout effects. The old result must already be stale at this point.
    ui.update({ value: '{"new":true}' }, false);
    replacing.complete('stale formatted body'); await tick();
    assert.equal(ui.changes.length, count, 'a worker must not overwrite an incoming external value before commit');
    ui.render();
    assert.equal(ui.view.state.doc.toString(), '{"new":true}');
    ui.format(); const parserChange = workers.at(-1);
    ui.update({ parser: 'html' });
    assert.equal(parserChange.terminated, true);
    parserChange.complete('late parser result'); await tick(); ui.render();
    assert.equal(ui.view.state.doc.toString(), '{"new":true}');
    ui.format(); const disabling = workers.at(-1);
    ui.update({ disabled: true });
    assert.equal(ui.view.state.facet(EditorState.readOnly), true);
    disabling.complete('late disabled result'); await tick(); ui.render();
    assert.equal(ui.view.state.doc.toString(), '{"new":true}');
    ui.update({ disabled: false }); ui.format(); const disposing = workers.at(-1);
    ui.unmount(); disposing.complete('late unmount result'); await tick();
    assert.equal(disposing.terminated, true);
    assert.equal(ui.view.destroyed, true);
  } finally { global.Worker = originalWorker; }
}
