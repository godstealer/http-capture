import { useEffect, useRef, useState } from 'react';
import { t } from './i18n';

export default function ImagePreview({ src }: { src: string }) {
  const viewport = useRef<HTMLDivElement>(null);
  const [bounds, setBounds] = useState({ width: 0, height: 0 });
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [zoom, setZoom] = useState<number | null>(null);
  useEffect(() => {
    setZoom(null); setSize({ width: 0, height: 0 });
    if (viewport.current) { viewport.current.scrollTop = 0; viewport.current.scrollLeft = 0; }
  }, [src]);
  useEffect(() => {
    const element = viewport.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setBounds({ width: entry.contentRect.width, height: entry.contentRect.height }));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const fit = size.width && size.height && bounds.width && bounds.height
    ? Math.min(1, Math.max(1, bounds.width - 32) / size.width, Math.max(1, bounds.height - 32) / size.height) : 1;
  const scale = zoom ?? fit;
  const changeZoom = (factor: number) => setZoom(Math.max(0.01, Math.min(8, scale * factor)));
  return <div className="image-preview">
    <div className="image-preview-toolbar">
      <button aria-pressed={zoom === null} onClick={() => setZoom(null)}>{t('适应窗口')}</button>
      <button aria-pressed={zoom === 1} onClick={() => setZoom(1)}>{t('原始尺寸')}</button>
      <button aria-label={t('缩小')} onClick={() => changeZoom(1 / 1.25)}>−</button>
      <span>{Math.round(scale * 100)}%</span>
      <button aria-label={t('放大')} onClick={() => changeZoom(1.25)}>+</button>
      {size.width > 0 && <span>{size.width} × {size.height}</span>}
    </div>
    <div className="image-preview-viewport" ref={viewport}>
      <div className="image-preview-canvas">
        <img src={src} alt={t('响应图片')} style={size.width ? { width: size.width * scale, height: size.height * scale } : { maxWidth: '100%' }}
          onLoad={event => setSize({ width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight })} />
      </div>
    </div>
  </div>;
}
