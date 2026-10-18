import { t } from './i18n';
import type { Theme } from './theme';

export default function ThemeSelector({ value, onChange }: { value: Theme; onChange: (theme: Theme) => void }) {
  const options: Array<[Theme, string]> = [
    ['light', t('浅色 · 琥珀')], ['dark', t('深色 · 薄荷')], ['cyberpunk', t('赛博 · 机甲')],
    ['arctic', t('极地蓝')], ['dusk', t('暮光紫')], ['paper', t('暖纸')],
    ['sonar', t('深海声呐')], ['forge', t('熔核机库')], ['matrix', t('矩阵终端')], ['sakura', t('樱雪未来')],
  ];
  return <label className="theme-picker"><select aria-label={t('界面主题')} value={value}
    onChange={event => onChange(event.target.value as Theme)}>
    {options.map(([id, label]) => <option value={id} key={id}>{label}</option>)}
  </select></label>;
}
