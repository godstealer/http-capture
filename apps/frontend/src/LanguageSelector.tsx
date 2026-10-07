import { t, useLanguage, type LanguagePreference } from './i18n';
export default function LanguageSelector() {
  const { preference, setLanguage } = useLanguage();
  return <label className="language-selector">{t('语言')} <select aria-label={t('界面语言')} value={preference} onChange={e => setLanguage(e.target.value as LanguagePreference)}>
    <option value="system">{t('跟随系统')}</option><option value="zh-CN">简体中文</option><option value="en">English</option>
  </select></label>;
}
