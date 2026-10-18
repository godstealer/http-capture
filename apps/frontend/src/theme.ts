export type Theme = 'light' | 'dark' | 'cyberpunk' | 'arctic' | 'dusk' | 'paper' | 'sonar' | 'forge' | 'matrix' | 'sakura';
const key = 'http-capture.theme';
const colorSchemes: Record<Theme, 'light' | 'dark'> = {
  light: 'light', dark: 'dark', cyberpunk: 'dark',
  arctic: 'light', dusk: 'dark', paper: 'light',
  sonar: 'dark', forge: 'dark', matrix: 'dark', sakura: 'light',
};
function validTheme(value: unknown): Theme {
  return typeof value === 'string' && Object.prototype.hasOwnProperty.call(colorSchemes, value)
    ? value as Theme : 'light';
}
export function readTheme(): Theme {
  try { return validTheme(localStorage.getItem(key)); }
  catch { return 'light'; }
}
export function applyTheme(theme: Theme) {
  const selected = validTheme(theme);
  document.documentElement.dataset.theme = selected;
  document.documentElement.dataset.colorScheme = colorSchemes[selected];
  try { localStorage.setItem(key, selected); } catch { /* Theme still works when storage is unavailable. */ }
}
