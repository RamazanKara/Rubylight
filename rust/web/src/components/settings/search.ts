import type { Category, Setting } from '../../lib/settings-types';

export function searchWords(query: string): string[] {
  return query.trim().toLowerCase().split(/\s+/).filter(Boolean);
}

export function matchesWords(text: string, words: readonly string[]): boolean {
  const lower = text.toLowerCase();
  return words.every((word) => lower.includes(word));
}

/** Search what users see, including choices inside menus and category context. */
export function matchesSetting(setting: Setting, category: Category, words: readonly string[]): boolean {
  const options = setting.control.kind === 'select'
    ? setting.control.options.flatMap((option) => [option.label, option.value])
    : [];
  return matchesWords([
    setting.label, setting.key, setting.key.replaceAll('_', ' '), setting.description,
    // Broad category descriptions mention unrelated choices (for example all
    // GPU vendors under Encoders), so use the category name and group instead.
    setting.group, setting.inactiveHint, category.label, category.id, ...options,
  ].join('\n'), words);
}
