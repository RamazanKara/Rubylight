import type { Category, Setting } from '../../lib/settings-types';

export function searchWords(query: string): string[] {
  return query.trim().toLowerCase().split(/\s+/).filter(Boolean);
}

export function matchesWords(text: string, words: readonly string[]): boolean {
  const lower = text.toLowerCase();
  return words.every((word) => lower.includes(word));
}

/**
 * Every setting the query finds, in schema order. A setting matches through
 * any of the categories it is shown under (its own and its settings group).
 */
export function searchSettings(
  settings: readonly Setting[],
  categoriesOf: (setting: Setting) => readonly Category[],
  query: string,
): Setting[] {
  const words = searchWords(query);
  if (!words.length) return [];
  return settings.filter((setting) => categoriesOf(setting).some((category) => matchesSetting(setting, category, words)));
}

/** Search what users see, including choices inside menus and category context. */
export function matchesSetting(setting: Setting, category: Category, words: readonly string[]): boolean {
  const options = setting.control.kind === 'select'
    ? setting.control.options.flatMap((option) => [option.label, option.value])
    : [];
  return matchesWords([
    setting.label, setting.key, setting.key.replaceAll('_', ' '), setting.description, setting.details,
    // Broad category descriptions mention unrelated choices (for example all
    // GPU vendors under Encoders), so use the category name and group instead.
    setting.group, setting.inactiveHint, category.label, category.id, ...options,
  ].join('\n'), words);
}
