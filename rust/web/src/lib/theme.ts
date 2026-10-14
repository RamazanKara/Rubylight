export type Theme = 'system' | 'light' | 'dark';

const KEY = 'butterpollo.theme';

// A browser that blocks site data throws on any use of localStorage; the
// console must still start, and the choice then lasts for this visit.
export function storedTheme(): Theme {
  try {
    const value = localStorage.getItem(KEY);
    return value === 'light' || value === 'dark' ? value : 'system';
  } catch {
    return 'system';
  }
}
export function applyTheme(theme: Theme) {
  document.documentElement.dataset.theme = theme;
  try {
    if (theme === 'system') localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, theme);
  } catch {
    // Not remembered; see storedTheme().
  }
}
