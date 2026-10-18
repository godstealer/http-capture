import { lazy, Suspense, useRef, useState } from "react";
import { t } from "./i18n";
import { parseLibrary, withSavedRequest, suggestedRequestName, type Library, type SavedRequestContext } from "./library";
import type { RequestDraft } from "./types";
import { emptyScripts } from "./ScriptsEditor";
import { Braces, FileCode2, FolderOpen } from 'lucide-react';
import LibraryCreateDialog from './LibraryCreateDialog';
import SaveRequestDialog from './SaveRequestDialog';
import './library-create.css';
type Creating = {kind:'collection'|'environment'|'request'|'script';collectionId?:string;draft?:RequestDraft;context?:SavedRequestContext};
const ScriptCodeEditor = lazy(() => import("./ScriptCodeEditor"));

export default function RequestLibrary({
  value,
  onChange,
  onSave,
  onReload,
  onOpen,
  draft,
  draftContext,
  message,
  busy,
}: {
  value: Library;
  onChange: (value: Library) => void;
  onSave: () => void;
  onReload: () => void;
  onOpen: (request: Library['requests'][number]) => void;
  draft?: RequestDraft;
  draftContext?: SavedRequestContext;
  message: string;
  busy: boolean;
}) {
  const [creating, setCreating] = useState<Creating|null>(null);
  const [reading, setReading] = useState(false);
  const importing = useRef(false);
  const latest = useRef({value,busy}); latest.current={value,busy};
  const menu = useRef<HTMLDetailsElement>(null);
  const locked = busy || reading;
  const [environment, setEnvironment] = useState("");
  const [variables, setVariables] = useState("{}");
  const [error, setError] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const update = (next: Library) => {
    onChange(next);
    setError("");
  };
  function openCreate(kind:Creating['kind'],collectionId?:string) {
    if(busy||importing.current)return;
    if((kind==='request'||kind==='script')&&!draft)return;
    if(menu.current)menu.current.open=false;
    setError('');
    setCreating({kind,collectionId,...((kind==='request'||kind==='script')?{draft:structuredClone(draft!),context:structuredClone(draftContext??{})}:{})});
  }
  function add(name:string) {
    if(!creating||busy||importing.current)return;
    const id=crypto.randomUUID();
    if(creating.kind==='collection')update({...value,collections:[...value.collections,{id,name}]});
    if(creating.kind==='environment'){
      update({...value,environments:[...value.environments,{id,name,variables:{}}]});
      setEnvironment(id);setVariables('{}');
    }
    if(creating.kind==='script'&&creating.draft)
      update({...value,scripts:[...value.scripts,{id,name,scripts:structuredClone(creating.draft.scripts??emptyScripts)}]});
    setCreating(null);
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
        disabled={locked}
        style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}
      >
        <div className="library-toolbar library-page-heading">
          <h1>{t("请求集合与环境")}</h1>
          <button disabled={busy} onClick={onSave}>
            {t("保存集合与环境")}
          </button>
          <details className="library-add-menu" ref={menu} onKeyDown={event=>{if(event.key==='Escape'&&menu.current){menu.current.open=false;menu.current.querySelector('summary')?.focus();}}}>
            <summary aria-disabled={locked} onClick={event=>{if(locked)event.preventDefault();}}>{t('新增')}</summary>
            <div className="library-add-options">
              <button type="button" disabled={locked} onClick={()=>openCreate('collection')}>{t('新建集合')}</button>
              <button type="button" disabled={locked} onClick={()=>openCreate('environment')}>{t('新建环境')}</button>
              <button type="button" disabled={locked||!draft} onClick={()=>openCreate('request')}>{t('收藏当前请求')}</button>
              <button type="button" disabled={locked||!draft} onClick={()=>openCreate('script')}>{t('保存当前脚本模板')}</button>
            </div>
          </details>
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
            if (!file || busy || importing.current) return;
            const snapshot=value;
            importing.current=true;setReading(true);
            try {
              const text=await file.text();
              if(latest.current.value!==snapshot||latest.current.busy)throw Error(t('集合已更新，请重新导入文件。'));
              update(parseLibrary(text,snapshot.revision));
            } catch (e) {setError(String(e));}
            finally {importing.current=false;setReading(false);}
          }}
        />
        <p>
          {t(
            "导入只替换当前编辑内容；点击保存才落盘。请求与环境可能包含凭据，请妥善保存导出文件。",
          )}
        </p>
        <div className="library-grid">
          <div className="library-collections">
            <div className="library-section-heading"><h2>{t('集合')}</h2><button type="button" onClick={()=>openCreate('collection')}>{t('新建集合')}</button></div>
            {!value.collections.length && <div className="secondary-empty"><FolderOpen size={28} aria-hidden="true"/><strong>{t('暂无请求集合')}</strong><p>{t('按项目整理常用请求，从新建一个集合开始。')}</p><button type="button" onClick={()=>openCreate('collection')}>{t('新建集合')}</button></div>}
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
                      <button onClick={() => onOpen(structuredClone(r))}>
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
          <div className="library-environments">
            <div className="library-section-heading"><h2>{t('编辑环境')}</h2><button type="button" onClick={()=>openCreate('environment')}>{t('新建环境')}</button></div>
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
            {!value.environments.length && <div className="secondary-empty"><Braces size={28} aria-hidden="true"/><strong>{t('暂无环境')}</strong><p>{t('新建环境后，可在这里维护请求变量。')}</p><button type="button" onClick={()=>openCreate('environment')}>{t('新建环境')}</button></div>}
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
        <details className="library-templates">
          <summary>{t("脚本模板")}</summary>
          {!value.scripts.length && <div className="secondary-empty"><FileCode2 size={28} aria-hidden="true"/><strong>{t('尚未保存脚本模板')}</strong><p>{t('打开请求后，可将脚本保存为模板。')}</p><button type="button" disabled={!draft} onClick={()=>openCreate('script')}>{t('保存当前脚本模板')}</button></div>}
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
      {creating?.kind==='request'&&creating.draft&&<SaveRequestDialog collections={value.collections}
        initialName={suggestedRequestName(creating.draft)} initialCollectionId={creating.collectionId}
        defaultCollectionName={t('我的请求')} busy={locked} error={error} confirmLabel={t('新增')}
        description={t('新增到当前编辑内容，点击「保存集合与环境」后写入磁盘。')}
        onClose={()=>setCreating(null)} onSave={selection=>{
          if(locked||importing.current)return;
          try{update(withSavedRequest(value,creating.draft!,selection,undefined,creating.context).library);setCreating(null);}
          catch(e){setError(String(e));}
        }}/>}
      {creating&&creating.kind!=='request'&&<LibraryCreateDialog busy={locked}
        title={creating.kind==='collection'?t('新建集合'):creating.kind==='environment'?t('新建环境'):t('保存当前脚本模板')}
        label={creating.kind==='collection'?t('集合名称'):creating.kind==='environment'?t('环境名称'):t('模板名称')}
        onCreate={add} onClose={()=>setCreating(null)}/>}
    </section>
  );
}
