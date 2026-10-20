// Exercise InterceptionEditor callbacks directly; child editors are stubs.
// Real CodeMirror history and worker cancellation are covered by test-body-format.cjs.
const assert = require("node:assert/strict");
const { gzipSync } = require("node:zlib");
const { buildSync } = require("esbuild");
const code = buildSync({
  entryPoints: ["apps/frontend/src/Interception.tsx"],
  bundle: true,
  platform: "node",
  format: "cjs",
  jsx: "automatic",
  write: false,
  loader: { ".css": "empty" },
  external: [
    "react",
    "react/jsx-runtime",
    "./clipboard",
    "./i18n",
    "./ScriptsEditor",
    "./api",
    "./types",
    "./OrderedHeaders",
    "./ResponseBody",
    "./BodyCodeEditor",
  ],
}).outputFiles[0].text;
const Body = () => null,
  Headers = () => null,
  Preview = () => null,
  readers = [];
global.FileReader = class {
  constructor() {
    readers.push(this);
  }
  readAsDataURL(file) {
    this.file = file;
  }
  abort() {
    this.aborted = true;
    this.onabort?.();
  }
  complete(text) {
    this.result =
      "data:application/json;base64," + Buffer.from(text).toString("base64");
    this.onload?.();
  }
};
function mount(stage = 'request') {
  const calls = [];
  let cursor = 0,
    dirty = true,
    mounted = true,
    tree;
  const hooks = [],
    effects = [];
  const react = {
    ...require("react"),
    useRef: (v) => (hooks[cursor++] ??= { current: v }),
    useState: (v) => {
      const cell = (hooks[cursor++] ??= {
        value: typeof v === "function" ? v() : v,
      });
      return [
        cell.value,
        (next) => {
          assert.ok(mounted, "no state after unmount");
          const value = typeof next === "function" ? next(cell.value) : next;
          if (!Object.is(value, cell.value)) {
            cell.value = value;
            dirty = true;
          }
        },
      ];
    },
    useLayoutEffect: (fn, deps) => {
      const cell = (hooks[cursor++] ??= {});
      if (!cell.deps || deps.some((v, i) => !Object.is(v, cell.deps[i]))) {
        cell.deps = deps;
        effects.push(() => {
          cell.cleanup?.();
          cell.cleanup = fn();
        });
      }
    },
  };
  const module = { exports: {} };
  new Function("require", "module", "exports", code)(
    (name) =>
      name === "react"
        ? react
        : name === "./i18n"
          ? { t: (x) => x }
          : name === "./types"
            ? { encodeText: (s) => Buffer.from(s).toString("base64") }
            : name === "./api"
              ? { invoke: async (...args) => { calls.push(args); } }
            : name === "./BodyCodeEditor"
              ? Body
              : name === "./OrderedHeaders"
                ? Headers
                : name === "./ResponseBody"
                  ? Preview
                  : name.startsWith("./")
                    ? {}
                    : require(name),
    module,
    module.exports,
  );
  const fields = [
    { name: "Content-Type", value: "application/json" },
    { name: "Content-Length", value: "1" },
    { name: "Content-Encoding", value: "gzip", disabled: true },
    { name: "X-Keep", value: "first" },
  ];
  const item = {
    id: "one",
    stage,
    flow: {
      request: {
        method: "POST",
        url: "https://example.test/",
        headers: fields,
        bodyBase64: Buffer.from('{"request":1}').toString("base64"),
      },
      response: {
        status: 200,
        version: "HTTP/2",
        headers: fields,
        bodyBase64: Buffer.from('{"response":1}').toString("base64"),
      },
    },
  };
  const nodes = (n) =>
    Array.isArray(n)
      ? n.flatMap(nodes)
      : n && typeof n === "object"
        ? [n, ...nodes(n.props?.children)]
        : [];
  function render() {
    while (dirty) {
      dirty = false;
      cursor = 0;
      tree = module.exports.InterceptionEditor({
        item,
        onDone: async () => {},
      });
      while (effects.length) effects.shift()();
    }
  }
  function find(fn) {
    render();
    const node = nodes(tree).find(fn);
    assert.ok(node, "expected element");
    return node.props;
  }
  render();
  return {
    calls,
    find,
    render,
    body: () => find((n) => n.type === Body),
    headers: () => find((n) => n.type === Headers),
    button: (text) =>
      find((n) => n.type === "button" && n.props.children === text),
    select: (label) => find((n) => n.props?.["aria-label"] === label),
    replace: (value) => {
      find((n) => n.type === "input" && n.props.type === "checkbox").onChange({
        target: { checked: value },
      });
      render();
    },
    file: (file) => {
      find((n) => n.type === "input" && n.props.type === "file").onChange({
        target: { files: [file], value: "chosen" },
      });
      render();
    },
    preview: () => find((n) => n.type === Preview),
    unmount: () => {
      hooks.forEach((c) => c?.cleanup?.());
      mounted = false;
    },
  };
}
(async () => {
  const response = mount('response');
  response.body().onChange('{"replacement":true}');
  response.render();
  response.button('替换响应并放行').onClick();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(response.calls[0][0], 'resolve_interception');
  assert.equal(response.calls[0][1].decision.action, 'modify');
  assert.equal(response.calls[0][1].decision.request, null);
  assert.equal(Buffer.from(response.calls[0][1].decision.response.bodyBase64, 'base64').toString(), '{"replacement":true}');
  response.button('原样放行').onClick();
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(response.calls[1][1].decision, {id:'one',action:'continue',request:null,response:null});
  response.unmount();
  const direct = mount();
  direct.replace(true);
  direct.button('返回自定义响应').onClick();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(direct.calls[0][1].decision.action, 'replace', 'request-stage direct replacement remains available');
  direct.unmount();
  for (const response of [false, true]) {
    const encoding = mount();
    if (response) encoding.replace(true);
    const originalType =
      'text/plain; profile="keep; charset=original"; charset=iso-8859-1; format=flowed';
    const fields = [
      { name: "X-Repeat", value: "first" },
      { name: "Content-Type", value: originalType },
      { name: "Content-Encoding", value: "gzip" },
      { name: "Content-Encoding", value: "br", disabled: true },
      { name: "Content-Length", value: "123" },
      { name: "Transfer-Encoding", value: "chunked" },
      { name: "Content-Length", value: "999", disabled: true },
      {
        name: "Content-Type",
        value: "text/plain; charset=latin1",
        disabled: true,
      },
      { name: "X-Repeat", value: "second" },
    ];
    encoding.headers().onChange(fields);
    encoding.render();
    encoding.select("拦截正文编码").onChange({ target: { value: "base64" } });
    encoding.render();
    const compressed = gzipSync("ASCII Latin-1 source").toString("base64");
    encoding.body().onChange(compressed);
    encoding.render();
    assert.equal(encoding.body().value, compressed);
    assert.deepEqual(
      encoding.headers().headers,
      fields.filter((_, index) => index !== 4 && index !== 5),
      "Base64 preserves compression, MIME, disabled fields and order while clearing framing",
    );
    // ASCII is also valid Latin-1. Rewriting it as UTF-8 must correct the old
    // charset, retaining the MIME type and unrelated quoted parameters.
    encoding
      .body()
      .onChange(Buffer.from("ASCII Latin-1 source").toString("base64"));
    encoding.render();
    encoding.select("拦截正文编码").onChange({ target: { value: "text" } });
    encoding.render();
    encoding.body().onChange("中文正文");
    encoding.render();
    assert.equal(encoding.body().value, "中文正文");
    assert.deepEqual(
      encoding.headers().headers,
      [
        fields[0],
        {
          ...fields[1],
          value:
            'text/plain; profile="keep; charset=original"; charset=utf-8; format=flowed',
        },
        fields[3],
        fields[6],
        fields[7],
        fields[8],
      ],
      "UTF-8 corrects enabled charset and compression without changing unrelated header semantics",
    );
    encoding.unmount();
  }
  const ui = mount();
  const oldHeaders = ui.headers();
  ui.body().onChange('{"edited":true}');
  ui.render();
  oldHeaders.onChange([
    { name: "X-Keep", value: "new" },
    { name: "Content-Length", value: "0", disabled: true },
  ]);
  ui.render();
  assert.equal(ui.body().value, '{"edited":true}');
  ui.body().onChange('{"merged":true}');
  ui.render();
  assert.deepEqual(ui.headers().headers, [
    { name: "X-Keep", value: "new" },
    { name: "Content-Length", value: "0", disabled: true },
  ]);
  const beforeRestore = ui.body();
  ui.button("恢复原始内容").onClick();
  ui.render();
  beforeRestore.onChange("stale formatter");
  ui.render();
  assert.equal(ui.body().value, '{"request":1}');
  const requestEditor = ui.body();
  ui.replace(true);
  requestEditor.onChange("stale request formatter");
  ui.render();
  assert.equal(ui.body().value, '{"response":1}');
  ui.replace(false);
  assert.equal(ui.body().value, '{"request":1}');
  ui.replace(true);
  const utfEditor = ui.body();
  ui.select("拦截正文编码").onChange({ target: { value: "base64" } });
  ui.render();
  utfEditor.onChange("stale UTF8 formatter");
  ui.render();
  assert.equal(
    ui.body().value,
    Buffer.from('{"response":1}').toString("base64"),
  );
  const binary = Buffer.from([0, 255, 128]).toString("base64");
  ui.body().onChange(binary);
  ui.render();
  const beforePreview = ui.body();
  ui.button("预览").onClick();
  ui.render();
  beforePreview.onChange("stale Base64 callback");
  ui.render();
  assert.equal(ui.preview().body, binary);
  ui.button("编辑").onClick();
  ui.render();
  assert.equal(ui.body().value, binary);
  ui.file({ size: 5, type: "application/json" });
  const first = readers.at(-1);
  ui.file({ size: 6, type: "application/json" });
  const second = readers.at(-1);
  assert.equal(first.aborted, true);
  second.complete('{"file":2}');
  await new Promise((r) => setImmediate(r));
  ui.render();
  assert.equal(ui.preview().body, Buffer.from('{"file":2}').toString("base64"));
  assert.ok(
    ui
      .headers()
      .headers.some((h) => h.name === "Content-Encoding" && h.disabled),
  );
  assert.ok(
    !ui
      .headers()
      .headers.some((h) => h.name === "Content-Length" && !h.disabled),
  );
  ui.file({ size: 5, type: "application/json" });
  const pending = readers.at(-1);
  ui.unmount();
  assert.equal(pending.aborted, true);
  await new Promise((r) => setImmediate(r));
  console.log(
    "Interception review: raw compression and UTF-8 charset consistency, stale callbacks, header merge, latest file replacement and unmount cancellation passed.",
  );
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
