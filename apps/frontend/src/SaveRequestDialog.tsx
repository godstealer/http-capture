import { useEffect, useId, useRef, useState, type FormEvent } from 'react';
import { createPortal } from 'react-dom';
import { t } from './i18n';
import type { SaveRequestSelection } from './library';
import './save-request.css';

type SaveRequestDialogProps = {
  collections: { id: string; name: string }[];
  initialName: string;
  initialCollectionId?: string;
  defaultCollectionName: string;
  busy: boolean;
  error: string;
  onSave: (selection: SaveRequestSelection) => void;
  onClose: () => void;
  description?: string;
  confirmLabel?: string;
};

export default function SaveRequestDialog({
  collections, initialName, initialCollectionId, defaultCollectionName, busy, error, onSave, onClose, description, confirmLabel,
}: SaveRequestDialogProps) {
  const id = useId();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  const collectionNameRef = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(initialName.slice(0, 128).replace(/[\uD800-\uDBFF]$/, ''));
  const [collectionId, setCollectionId] = useState(() =>
    collections.some(collection => collection.id && collection.id === initialCollectionId)
      ? initialCollectionId!
      : collections.find(collection => collection.id)?.id ?? '',
  );
  const [collectionName, setCollectionName] = useState(defaultCollectionName.slice(0, 128));
  const [creating, setCreating] = useState(!collections.some(collection => collection.id));
  const [validation, setValidation] = useState<{ field: 'name' | 'collectionName'; message: string } | null>(null);
  const existingCollections = collections.filter(collection => collection.id);
  const selectedCollectionId = existingCollections.some(collection => collection.id === collectionId)
    ? collectionId : existingCollections[0]?.id ?? '';
  const creatingCollection = creating || !selectedCollectionId;
  const message = validation?.message || error;

  useEffect(() => {
    const dialog = dialogRef.current!;
    const previousFocus = document.activeElement;
    if (!dialog.open) dialog.showModal();
    nameRef.current?.focus();
    nameRef.current?.select();
    return () => {
      if (dialog.open) dialog.close();
      if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus();
    };
  }, []);

  function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy) return;
    if (!name.trim()) {
      setValidation({ field: 'name', message: t('请输入请求名称。') });
      nameRef.current?.focus();
      return;
    }
    if (creatingCollection && !collectionName.trim()) {
      setValidation({ field: 'collectionName', message: t('请输入集合名称。') });
      collectionNameRef.current?.focus();
      return;
    }
    setValidation(null);
    onSave(!creatingCollection
      ? { name: name.trim(), collectionId: selectedCollectionId }
      : { name: name.trim(), collectionName: collectionName.trim() });
  }

  return createPortal(<dialog ref={dialogRef} className="save-request-dialog"
    aria-labelledby={`${id}-title`} aria-describedby={`${id}-description`}
    onCancel={event => { event.preventDefault(); if (!busy) onClose(); }}>
    <form onSubmit={save} noValidate aria-busy={busy}>
      <header className="save-request-heading">
        <h2 id={`${id}-title`}>{t('保存请求')}</h2>
        <p id={`${id}-description`}>{description ?? t('保存后可从左侧「集合」再次打开。')}</p>
      </header>
      <div className="save-request-fields">
        <label htmlFor={`${id}-name`}>{t('请求名称')}</label>
        <input ref={nameRef} id={`${id}-name`} name="requestName" value={name} required maxLength={128}
          autoFocus autoComplete="off" disabled={busy} aria-invalid={validation?.field === 'name' || undefined}
          aria-describedby={validation?.field === 'name' ? `${id}-error` : undefined}
          onChange={event => { setName(event.target.value); setValidation(null); }} />
        {!!existingCollections.length && <label htmlFor={`${id}-collection`}>{t('保存到集合')}</label>}
        {!!existingCollections.length && <div className="save-request-collection-picker">
          <select id={`${id}-collection`} name="collectionId" value={selectedCollectionId} disabled={busy || creatingCollection}
            onChange={event => { setCollectionId(event.target.value); setValidation(null); }}>
            {existingCollections.map(collection => <option key={collection.id} value={collection.id}>{collection.name}</option>)}
          </select>
          <button type="button" disabled={busy} onClick={() => {
            setCreating(!creatingCollection); setValidation(null);
            if (!creatingCollection) queueMicrotask(() => { collectionNameRef.current?.focus(); collectionNameRef.current?.select(); });
          }}>{creatingCollection ? t('使用已有集合') : t('新建集合')}</button>
        </div>}
        {creatingCollection && <>
          {!existingCollections.length && <p className="save-request-new-hint">{t('还没有集合，将同时创建下面的集合。')}</p>}
          <label htmlFor={`${id}-collection-name`}>{t('集合名称')}</label>
          <input ref={collectionNameRef} id={`${id}-collection-name`} name="collectionName" value={collectionName}
            required maxLength={128} autoComplete="off" disabled={busy}
            aria-invalid={validation?.field === 'collectionName' || undefined}
            aria-describedby={validation?.field === 'collectionName' ? `${id}-error` : undefined}
            onChange={event => { setCollectionName(event.target.value); setValidation(null); }} />
        </>}
      </div>
      {message && <p id={`${id}-error`} className="save-request-error" role="alert">{message}</p>}
      <footer className="save-request-actions">
        <button type="button" onClick={onClose} disabled={busy}>{t('取消')}</button>
        <button type="submit" className="save-request-confirm" disabled={busy}>
          {busy ? t('正在保存…') : confirmLabel ?? t('保存')}
        </button>
      </footer>
    </form>
  </dialog>, document.body);
}
