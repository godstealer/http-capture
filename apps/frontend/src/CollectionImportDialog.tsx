import { useRef, useState } from 'react';
import { FileJson2, FolderInput, Terminal, X } from 'lucide-react';
import { t } from './i18n';
import { parseExternalImport } from './externalCollectionImport';
import type { ExternalImportFormat, ExternalImportPlan } from './externalImportTypes';

const sources: { id: ExternalImportFormat; label: string; detail: string }[] = [
  { id: 'postman', label: 'Postman', detail: 'Collection v2.1 JSON' },
  { id: 'hoppscotch', label: 'Hoppscotch', detail: 'Collection JSON' },
  { id: 'openapi', label: 'OpenAPI 3', detail: 'Swagger export JSON' },
  { id: 'insomnia', label: 'Insomnia', detail: 'Export v4 JSON' },
  { id: 'har', label: 'HAR', detail: 'HTTP Archive JSON' },
  { id: 'curl', label: 'cURL', detail: '单条命令' },
];

export { sources as externalImportSources };

export default function CollectionImportDialog({ initialFormat, busy, onClose, onValidate, onApply }: {
  initialFormat: ExternalImportFormat;
  busy: boolean;
  onClose: () => void;
  onValidate: (plan: ExternalImportPlan) => void;
  onApply: (plan: ExternalImportPlan) => void;
}) {
  const [format, setFormat] = useState<ExternalImportFormat>(initialFormat);
  const [inputText, setInputText] = useState('');
  const [fileName, setFileName] = useState('');
  const [reading, setReading] = useState(false);
  const [plan, setPlan] = useState<ExternalImportPlan | null>(null);
  const [error, setError] = useState('');
  const generation = useRef(0);
  const requestCount = plan?.collections.reduce((count, collection) => count + collection.requests.length, 0) ?? 0;

  function changeFormat(next: ExternalImportFormat) {
    generation.current++;
    setFormat(next);
    setInputText('');
    setFileName('');
    setReading(false);
    setPlan(null);
    setError('');
  }

  function preview(text: string, selectedFormat = format) {
    try {
      const result = parseExternalImport(text, selectedFormat);
      const count = result.collections.reduce((sum, item) => sum + item.requests.length, 0);
      if (!count && !(result.environments?.length)) throw Error(t('文件中没有可导入的请求或环境。'));
      onValidate(result);
      setPlan(result);
      setError('');
    } catch (cause) {
      setPlan(null);
      setError(String(cause));
    }
  }

  async function readFile(file: File) {
    const version = ++generation.current;
    setPlan(null);
    setError('');
    setFileName(file.name);
    if (file.size > 8 * 1024 * 1024) {
      setError(t('文件超过 8 MiB，请先从来源工具导出较小的集合。'));
      return;
    }
    setReading(true);
    try {
      const contents = await file.text();
      if (version !== generation.current) return;
      setInputText(contents);
      preview(contents);
    } catch (cause) {
      if (version === generation.current) setError(String(cause));
    } finally {
      if (version === generation.current) setReading(false);
    }
  }

  return <div className="modal-backdrop external-import-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !reading) onClose(); }} onKeyDown={event => { if (event.key === 'Escape' && !reading) onClose(); }}>
    <div className="modal external-import-modal" role="dialog" aria-modal="true" aria-labelledby="external-import-title">
      <button className="modal-close icon-button" type="button" aria-label={t('关闭')} onClick={onClose} disabled={reading}><X size={18}/></button>
      <div className="external-import-title"><FolderInput size={23} aria-hidden="true"/><div><h2 id="external-import-title">{t('导入外部集合')}</h2><p>{t('从其他工具的导出文件中提取请求，预览后追加到当前集合。')}</p></div></div>
      <fieldset disabled={busy || reading}>
        <div className="external-import-sources" role="group" aria-label={t('来源格式')}>
          {sources.map(source => <button key={source.id} type="button" className={format === source.id ? 'is-selected' : ''} aria-pressed={format === source.id} onClick={() => changeFormat(source.id)}>
            {source.id === 'curl' ? <Terminal size={16} aria-hidden="true"/> : <FileJson2 size={16} aria-hidden="true"/>}<span><strong>{source.label}</strong><small>{t(source.detail)}</small></span>
          </button>)}
        </div>
        <div className="external-import-input">
          <label className="external-import-file">{t('选择导出文件')}<input type="file" accept={format === 'curl' ? '.txt,.curl,text/plain' : '.json,.har,application/json'} onChange={event => { const file = event.target.files?.[0]; event.target.value = ''; if (file) void readFile(file); }}/></label>
          <span>{fileName || t('也可以在下方粘贴内容')}</span>
          <textarea aria-label={t('导入内容')} spellCheck={false} value={inputText} onChange={event => { generation.current++; setInputText(event.target.value); setFileName(''); setPlan(null); setError(''); }} placeholder={format === 'curl' ? "curl 'https://example.com' -H 'Accept: application/json'" : t('粘贴来源工具导出的 JSON 内容')}/>
          <button type="button" disabled={!inputText.trim()} onClick={() => preview(inputText)}>{t('预览导入')}</button>
        </div>
      </fieldset>
      {reading && <p role="status">{t('正在读取文件…')}</p>}
      {error && <p className="external-import-error" role="alert">{error}</p>}
      {plan && <div className="external-import-preview" role="status">
        <strong>{t('预览：{v0} 个集合，{v1} 个请求', { v0: plan.collections.length, v1: requestCount })}</strong>
        {plan.environments?.length ? <span>{t('另有 {v0} 个环境', { v0: plan.environments.length })}</span> : null}
        <ul>{plan.collections.map((collection, index) => <li key={index}>{collection.name} · {collection.requests.length} {t('个请求')}</li>)}</ul>
        {plan.warnings.length > 0 && <div className="external-import-warnings"><strong>{t('转换提示（请确认后再导入）')}</strong><ul>{plan.warnings.map((warning, index) => <li key={index}>{warning}</li>)}</ul></div>}
      </div>}
      <p className="external-import-help">{t('导入只追加到当前编辑内容，点击“保存集合与环境”后才会写入磁盘。原文件不会上传。ApiFox、ApiPost 和 Paw 请先导出卡片指定的标准格式。')}</p>
      <div className="external-import-actions"><button type="button" onClick={onClose} disabled={reading}>{t('取消')}</button><button className="button primary" type="button" onClick={() => { if (plan) try { onApply(plan); } catch (cause) { setError(String(cause)); } }} disabled={!plan || busy || reading}>{t('添加到集合')}</button></div>
    </div>
  </div>;
}
