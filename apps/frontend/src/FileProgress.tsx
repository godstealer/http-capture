import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { t } from './i18n';

/** A modal dialog also blocks keyboard interaction while a file operation runs. */
export function FileProgress({ message }: { message: string }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current!;
    dialog.showModal();
    return () => dialog.close();
  }, []);
  return createPortal(<dialog ref={ref} className="modal file-progress" aria-label={t('文件操作')}
    onCancel={event => event.preventDefault()}>
    <h2>{t('正在处理，请稍候')}</h2>
    <p role="status" aria-live="polite">{message || t('处理中…')}</p>
    <progress aria-label={message || t('处理中…')} />
  </dialog>, document.body);
}
