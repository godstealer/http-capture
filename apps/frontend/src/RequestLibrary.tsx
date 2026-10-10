import { lazy, Suspense, useRef, useState } from "react";
import { t } from "./i18n";
import { parseLibrary, type Library } from "./library";
import type { RequestDraft } from "./types";
import { emptyScripts } from "./ScriptsEditor";
const ScriptCodeEditor = lazy(() => import("./ScriptCodeEditor"));

export default function RequestLibrary({
  value,
  onChange,
  onSave,
  onReload,
  onOpen,
  draft,
  message,
  busy,
}: {
  value: Library;
  onChange: (value: Library) => void;
  onSave: () => void;
  onReload: () => void;
  onOpen: (draft: RequestDraft) => void;
  draft?: RequestDraft;
  message: string;
  busy: boolean;
}) {
  const [name, setName] = useState("");
  const [collection, setCollection] = useState("");
  const [environment, setEnvironment] = useState("");
  const [variables, setVariables] = useState("{}");
  const [error, setError] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const update = (next: Library) => {
    onChange(next);
    setError("");
  };
  function add(kind: "collection" | "request" | "environment" | "script") {
    if (!name.trim()) {
      setError(t("请输入名称"));
      return;
    }
    const id = crypto.randomUUID();
    if (kind === "collection") {
      update({
        ...value,
        collections: [...value.collections, { id, name: name.trim() }],
      });
      setCollection(id);
    }
    if (kind === "request" && draft) {
      if (!value.collections.some((c) => c.id === collection)) {
        setError(t("请选择集合"));
        return;
      }
      update({
        ...value,
        requests: [
          ...value.requests,
          {
            id,
            name: name.trim(),
            collectionId: collection,
            draft: structuredClone(draft),
          },
        ],
      });
    }
    if (kind === "environment") {
      update({
        ...value,
        environments: [
          ...value.environments,
          { id, name: name.trim(), variables: {} },
        ],
      });
      setEnvironment(id);
      setVariables("{}");
    }
    if (kind === "script" && draft)
      update({
        ...value,
        scripts: [
          ...value.scripts,
          {
            id,
            name: name.trim(),
            scripts: structuredClone(draft.scripts ?? emptyScripts),
          },
        ],
      });
    setName("");
  }
  function exportFile() {
    const url = URL.createObjectURL(
      new Blob(
        [
          JSON.stringify(
            { format: "http-capture-library", version: 1, library: value },
            null,
            2,
          ),
        ],
        { type: "application/json" },
      ),
    );
    const link = document.createElement("a");
    link.href = url;
    link.download = "request-library.json";
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 30000);
  }
  return (
    <section className="request-library">
      <fieldset
        disabled={busy}
        style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}
      >
        <div className="library-toolbar">
          <strong>{t("请求集合与环境")}</strong>
          <button disabled={busy} onClick={onSave}>
            {t("保存集合与环境")}
          </button>
          <button
            disabled={busy}
            onClick={() => {
              if (window.confirm(t("重新加载会丢弃未保存的集合修改，继续？")))
                onReload();
            }}
          >
            {t("重新加载")}
          </button>
          <button onClick={exportFile}>{t("导出集合与环境")}</button>
          <button onClick={() => input.current?.click()}>
            {t("导入集合与环境")}
          </button>
          <span role="status">{message}</span>
        </div>
        <input
          hidden
          ref={input}
          type="file"
          accept=".json"
          onChange={async (e) => {
            const file = e.target.files?.[0];
            e.target.value = "";
            if (!file) return;
            try {
              update(parseLibrary(await file.text(), value.revision));
            } catch (e) {
              setError(String(e));
            }
          }}
        />
        <p>
          {t(
            "导入只替换当前编辑内容；点击保存才落盘。请求与环境可能包含凭据，请妥善保存导出文件。",
          )}
        </p>
        <div className="library-toolbar">
          <input
            aria-label={t("名称")}
            placeholder={t("名称")}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <button onClick={() => add("collection")}>{t("新建集合")}</button>
          <button onClick={() => add("environment")}>{t("新建环境")}</button>
          <select
            aria-label={t("集合")}
            value={collection}
            onChange={(e) => setCollection(e.target.value)}
          >
            <option value="">{t("请选择集合")}</option>
            {value.collections.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
          <button disabled={!draft} onClick={() => add("request")}>
            {t("收藏当前请求")}
          </button>
          <button disabled={!draft} onClick={() => add("script")}>
            {t("保存当前脚本模板")}
          </button>
        </div>
        <div className="library-grid">
          <div>
            {value.collections.map((c) => (
              <details key={c.id} open>
                <summary>
                  <input
                    aria-label={t("集合名称")}
                    value={c.name}
                    onChange={(e) =>
                      update({
                        ...value,
                        collections: value.collections.map((v) =>
                          v.id === c.id ? { ...v, name: e.target.value } : v,
                        ),
                      })
                    }
                  />
                  <button
                    onClick={() =>
                      update({
                        ...value,
                        collections: value.collections.filter(
                          (v) => v.id !== c.id,
                        ),
                        requests: value.requests.filter(
                          (v) => v.collectionId !== c.id,
                        ),
                      })
                    }
                  >
                    {t("移除")}
                  </button>
                </summary>
                {value.requests
                  .filter((r) => r.collectionId === c.id)
                  .map((r) => (
                    <div key={r.id} className="library-toolbar">
                      <input
                        aria-label={t("请求名称")}
                        value={r.name}
                        onChange={(e) =>
                          update({
                            ...value,
                            requests: value.requests.map((v) =>
                              v.id === r.id
                                ? { ...v, name: e.target.value }
                                : v,
                            ),
                          })
                        }
                      />
                      <span>{r.draft.method}</span>
                      <button onClick={() => onOpen(structuredClone(r.draft))}>
                        {t("打开请求")}
                      </button>
                      <button
                        onClick={() =>
                          update({
                            ...value,
                            requests: value.requests.filter(
                              (v) => v.id !== r.id,
                            ),
                          })
                        }
                      >
                        {t("移除")}
                      </button>
                    </div>
                  ))}
              </details>
            ))}
          </div>
          <div>
            <select
              aria-label={t("编辑环境")}
              value={environment}
              onChange={(e) => {
                setEnvironment(e.target.value);
                setVariables(
                  JSON.stringify(
                    value.environments.find((v) => v.id === e.target.value)
                      ?.variables ?? {},
                    null,
                    2,
                  ),
                );
              }}
            >
              <option value="">{t("请选择环境")}</option>
              {value.environments.map((v) => (
                <option key={v.id} value={v.id}>
                  {v.name}
                </option>
              ))}
            </select>
            {value.environments.some((v) => v.id === environment) && (
              <>
                <input
                  aria-label={t("环境名称")}
                  value={
                    value.environments.find((v) => v.id === environment)!.name
                  }
                  onChange={(e) =>
                    update({
                      ...value,
                      environments: value.environments.map((v) =>
                        v.id === environment
                          ? { ...v, name: e.target.value }
                          : v,
                      ),
                    })
                  }
                />
                <Suspense fallback={<pre>{variables}</pre>}>
                  <ScriptCodeEditor
                    value={variables}
                    onChange={setVariables}
                    stage="variables"
                    variables={{}}
                  />
                </Suspense>
                <button
                  onClick={() => {
                    try {
                      const parsed = JSON.parse(variables);
                      if (
                        !parsed ||
                        Array.isArray(parsed) ||
                        typeof parsed !== "object" ||
                        Object.values(parsed).some((v) => typeof v !== "string")
                      )
                        throw Error(t("变量必须是字符串值的 JSON 对象"));
                      update({
                        ...value,
                        environments: value.environments.map((v) =>
                          v.id === environment
                            ? { ...v, variables: parsed }
                            : v,
                        ),
                      });
                    } catch (e) {
                      setError(String(e));
                    }
                  }}
                >
                  {t("应用变量")}
                </button>
                <button
                  onClick={() => {
                    update({
                      ...value,
                      environments: value.environments.filter(
                        (v) => v.id !== environment,
                      ),
                    });
                    setEnvironment("");
                  }}
                >
                  {t("移除环境")}
                </button>
              </>
            )}
            <p>
              {t(
                "替换顺序：环境变量 → 请求变量覆盖。URL、请求头及 UTF-8 正文支持 {{name}}；未定义变量阻止发送。",
              )}
            </p>
          </div>
        </div>
        <details>
          <summary>{t("脚本模板")}</summary>
          {value.scripts.map((v) => (
            <div className="library-toolbar" key={v.id}>
              <input
                aria-label={t("模板名称")}
                value={v.name}
                onChange={(e) =>
                  update({
                    ...value,
                    scripts: value.scripts.map((s) =>
                      s.id === v.id ? { ...s, name: e.target.value } : s,
                    ),
                  })
                }
              />
              <button
                onClick={() =>
                  update({
                    ...value,
                    scripts: value.scripts.filter((s) => s.id !== v.id),
                  })
                }
              >
                {t("移除")}
              </button>
            </div>
          ))}
        </details>
        {error && <p role="alert">{error}</p>}
      </fieldset>
    </section>
  );
}
