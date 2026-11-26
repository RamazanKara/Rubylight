// Pure helpers, storage, scroll-spy and dialog guards for the All settings page.
import type { Config, ConfigValue, Metadata } from '../../lib/api';
import { confirm } from '../../lib/feedback.svelte';
import { guardNavigation } from '../../lib/router.svelte';
import { GAMES_PAGE_KEY_SET } from '../../lib/schema/tiers';
import {
  categoriesOf,
  groupAnchor,
  placeOf,
  topicGroup,
  topicTab,
  type LayoutGroup,
  type SettingsTab,
  type TabId,
} from '../../lib/settings-schema';
import type { Setting } from '../../lib/settings-types';
import { matchesSetting, matchesWords } from './search';
import { differsFromDefault, effective, has, normalise, NOT_SETTINGS, toText, type Edits } from './values';

// Configuration keys are for people editing the settings file or following
// a guide; the choice is remembered in this browser.
const SHOW_KEYS_STORAGE = 'butterpollo.settings.show-keys';

export function readShowKeys(): boolean {
  try {
    return localStorage.getItem(SHOW_KEYS_STORAGE) === 'true';
  } catch {
    return false;
  }
}

export function writeShowKeys(show: boolean): void {
  try {
    if (show) localStorage.setItem(SHOW_KEYS_STORAGE, 'true');
    else localStorage.removeItem(SHOW_KEYS_STORAGE);
  } catch {
    // Storage may be unavailable (private windows); the switch still works for this visit.
  }
}

export function isSettingVisible(setting: Setting, values: Record<string, ConfigValue>): boolean {
  if (!setting.visibleWhen) return true;
  try {
    return setting.visibleWhen(values);
  } catch {
    return true;
  }
}

/** Empty although the current choices make the host need a value. */
export function isSettingMissing(setting: Setting, values: Record<string, ConfigValue>): boolean {
  const value = values[setting.key];
  if (value !== null && value !== undefined && String(value).trim() !== '') return false;
  try {
    return setting.requiredWhen?.(values) ?? false;
  } catch {
    return false;
  }
}

export function countChangedByTab(
  pending: ReadonlySet<string>,
  byKey: ReadonlyMap<string, Setting>,
): Partial<Record<TabId, number>> {
  const counts: Partial<Record<TabId, number>> = {};
  for (const key of pending) {
    const found = byKey.get(key);
    const id = found ? topicTab(found) : 'advanced';
    counts[id] = (counts[id] ?? 0) + 1;
  }
  return counts;
}

/** Stored or newly added keys the schema does not describe. */
export function findUnknownKeys(config: Config, edits: Edits, byKey: ReadonlyMap<string, Setting>): string[] {
  return [...new Set([...Object.keys(config), ...Object.keys(edits)])]
    .filter((key) => !byKey.has(key) && !NOT_SETTINGS.has(key))
    .sort();
}

export function buildTabSections(
  tabId: TabId,
  tabLabel: string,
  tabGroups: LayoutGroup[],
  hasDebug: boolean,
  names: { security: string; debug: string; integrations: string; customKeys: string },
): string[] {
  return [
    ...(tabId === 'advanced' ? [names.security] : []),
    ...tabGroups.map((group) => group.name || tabLabel),
    ...(hasDebug ? [names.debug] : []),
    ...(tabId === 'picture' ? [names.integrations] : []),
    ...(tabId === 'advanced' ? [names.customKeys] : []),
  ];
}

/** The NVIDIA components panel: on a PC with an NVIDIA graphics card, or wherever they are installed. */
export function isNvidiaPanelShown(metadata: Metadata | null): boolean {
  const component = metadata?.nvidia_enhancements?.component;
  return !!component && (component.nvidia_gpu || component.installed || component.state !== 'missing');
}

export function findAnchorLauncherGroup(anchor: string, tabId: TabId, described: Setting[]): string | null {
  if (!anchor) return null;
  const launcher = described.find((item) => GAMES_PAGE_KEY_SET.has(item.key) && groupAnchor(topicGroup(item)) === anchor);
  return launcher && topicTab(launcher) === tabId ? topicGroup(launcher) : null;
}

export const isPlainClick = (event: MouseEvent): boolean =>
  event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;

export interface RowModel {
  part: Setting | undefined;
  value: ConfigValue;
  stored: boolean;
  changed: boolean;
  resetting: boolean;
  differs: boolean;
  fixed:
    | {
        setting: Setting;
        value: ConfigValue;
        missing: boolean;
        invalidText: string | undefined;
        onchange: (value: ConfigValue) => void;
        oninvalid: (text: string | null) => void;
      }
    | undefined;
  pnp: { value: string; onchange: (value: string | null) => void } | undefined;
  daily: { value: string; onchange: (value: string | null) => void } | undefined;
}

export function buildRowModel(
  setting: Setting,
  defaults: ReadonlyMap<string, Setting>,
  edits: Edits,
  config: Config,
  changes: Record<string, ConfigValue>,
  invalid: Record<string, string>,
  values: Record<string, ConfigValue>,
  actions: {
    edit: (key: string, value: ConfigValue) => void;
    reset: (key: string) => void;
    setInvalid: (key: string, text: string | null) => void;
  },
): RowModel {
  const dailySetting =
    setting.control.kind === 'number' && setting.control.daily ? defaults.get(setting.control.daily.key) : undefined;
  const fixedSetting =
    setting.control.kind === 'select' && setting.control.fixed ? defaults.get(setting.control.fixed.key) : undefined;
  const pnpSetting =
    setting.control.kind === 'adapter' && setting.control.pnp ? defaults.get(setting.control.pnp) : undefined;
  const part = dailySetting ?? fixedSetting ?? pnpSetting;
  return {
    part,
    value: normalise(setting, effective(setting, edits, config)),
    stored: has(config, setting.key) || (!!part && has(config, part.key)),
    changed: has(changes, setting.key) || (!!part && has(changes, part.key)),
    resetting: edits[setting.key] === null && has(config, setting.key),
    differs: differsFromDefault(setting, edits, config) || (!!part && differsFromDefault(part, edits, config)),
    fixed: fixedSetting
      ? {
          setting: fixedSetting,
          value: normalise(fixedSetting, effective(fixedSetting, edits, config)),
          missing: isSettingMissing(fixedSetting, values),
          invalidText: invalid[fixedSetting.key],
          onchange: (value) => actions.edit(fixedSetting.key, value),
          oninvalid: (text) => actions.setInvalid(fixedSetting.key, text),
        }
      : undefined,
    pnp: pnpSetting
      ? {
          value: toText(normalise(pnpSetting, effective(pnpSetting, edits, config))),
          onchange: (value) => (value === null ? actions.reset(pnpSetting.key) : actions.edit(pnpSetting.key, value)),
        }
      : undefined,
    daily:
      part && part === dailySetting
        ? {
            value: toText(normalise(part, effective(part, edits, config))),
            onchange: (value) => (value === null ? actions.reset(part.key) : actions.edit(part.key, value)),
          }
        : undefined,
  };
}

export interface TabSearchResult {
  tab: SettingsTab;
  settings: Setting[];
  unknown: string[];
}

export function buildSearchResults(
  tabs: readonly SettingsTab[],
  described: Setting[],
  unknownKeys: string[],
  words: string[],
  inReview: (key: string) => boolean,
  listed: (setting: Setting) => Setting,
): TabSearchResult[] {
  return tabs
    .map((tab) => ({
      tab,
      settings: [
        ...new Set(
          described
            .filter(
              (setting) =>
                placeOf(setting).tab === tab.id &&
                inReview(setting.key) &&
                categoriesOf(setting).some((shownUnder) => matchesSetting(setting, shownUnder, words)),
            )
            .map(listed),
        ),
      ],
      unknown: tab.id === 'advanced' ? unknownKeys.filter((key) => inReview(key) && matchesWords(key, words)) : [],
    }))
    .filter((result) => result.settings.length > 0 || result.unknown.length > 0);
}

export function countSearchResults(results: TabSearchResult[]): number {
  return new Set(results.flatMap((result) => [...result.settings.map((setting) => setting.key), ...result.unknown])).size;
}

export function customizedReviewKeys(
  described: Setting[],
  edits: Edits,
  config: Config,
  pending: ReadonlySet<string>,
  unknownKeys: string[],
): string[] {
  return [
    ...described
      .filter((setting) => differsFromDefault(setting, edits, config) || pending.has(setting.key))
      .map((setting) => setting.key),
    ...unknownKeys,
  ];
}

export function measureStickyHeader(): number {
  // The console's navigation sticks to the top of narrow windows; the contents stick below it.
  const aside = document.querySelector('aside');
  return aside && getComputedStyle(aside).position === 'sticky' ? Math.round(aside.getBoundingClientRect().height) : 0;
}

/** The section at the top of the window, under the sticky bars. */
export function activeSection(
  frame: HTMLElement | undefined,
  sections: string[],
  headerHeight: number,
  contentsHeight: number,
): string | null {
  if (!frame) return null;
  const line = headerHeight + contentsHeight + 24;
  let found = sections[0] ?? '';
  for (const name of sections) {
    const element = document.getElementById(groupAnchor(name));
    if (element && element.getBoundingClientRect().top <= line) found = name;
  }
  return found;
}

export function scrollSectionIntoTrack(track: HTMLElement | undefined, current: string): void {
  const link = current && track?.querySelector<HTMLElement>(`[data-section="${CSS.escape(current)}"]`);
  if (!link || !track) return;
  const { left, right } = link.getBoundingClientRect();
  const bar = track.getBoundingClientRect();
  if (left < bar.left || right > bar.right - 32) track.scrollLeft += left - bar.left - 16;
}

export function observeStickyHeader(onMeasure: () => void, onSpy: () => void): () => void {
  onMeasure();
  // The header grows while its menu is open on a narrow window.
  const aside = document.querySelector('aside');
  const observer = typeof ResizeObserver === 'function' && aside ? new ResizeObserver(() => onMeasure()) : null;
  if (aside) observer?.observe(aside);
  let frameRequest = 0;
  const onScroll = () => {
    cancelAnimationFrame(frameRequest);
    frameRequest = requestAnimationFrame(onSpy);
  };
  const onResize = () => {
    onMeasure();
    onScroll();
  };
  window.addEventListener('scroll', onScroll, { passive: true });
  window.addEventListener('resize', onResize);
  return () => {
    cancelAnimationFrame(frameRequest);
    observer?.disconnect();
    window.removeEventListener('scroll', onScroll);
    window.removeEventListener('resize', onResize);
  };
}

export const prefersReducedMotion = (): boolean =>
  typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

/** On arrival, keep a section in place while the panels above it finish loading, until the user scrolls. */
export function holdSectionInView(frame: HTMLElement | undefined, element: HTMLElement): void {
  if (typeof ResizeObserver !== 'function' || !frame) return;
  const observer = new ResizeObserver(() => element.scrollIntoView({ block: 'start' }));
  const events = ['wheel', 'touchstart', 'keydown', 'pointerdown'] as const;
  const stop = () => {
    observer.disconnect();
    clearTimeout(timer);
    for (const name of events) window.removeEventListener(name, stop, true);
  };
  const timer = setTimeout(stop, 2000);
  for (const name of events) window.addEventListener(name, stop, { capture: true, passive: true });
  observer.observe(frame);
}

export function scrollSectionIntoView(frame: HTMLElement | undefined, name: string, smooth: boolean): boolean {
  const element = document.getElementById(groupAnchor(name));
  if (!element) return false;
  element.scrollIntoView({ block: 'start', behavior: smooth && !prefersReducedMotion() ? 'smooth' : 'auto' });
  if (!smooth) holdSectionInView(frame, element);
  return true;
}

/** Focus and highlight a single setting row; returns false when the row is hidden. */
export function focusSettingRow(key: string): boolean {
  const row = document.querySelector<HTMLElement>(`[data-setting="${CSS.escape(key)}"]`);
  if (!row) return false;
  row.scrollIntoView({ block: 'center' });
  row.querySelector<HTMLElement>('input, select, textarea, button')?.focus({ preventScroll: true });
  const mark = getComputedStyle(row).getPropertyValue('--accent-soft').trim();
  if (mark && !prefersReducedMotion()) {
    row.animate([{ backgroundColor: mark }, { backgroundColor: 'transparent' }], { duration: 1800, easing: 'ease-out' });
  }
  return true;
}

export function confirmDiscardChanges(count: number): Promise<boolean> {
  return confirm({
    title: 'Discard unsaved changes?',
    message: `${count} unsaved ${count === 1 ? 'change is' : 'changes are'} lost. Rubylight keeps its current settings.`,
    confirm: 'Discard',
    danger: true,
  });
}

export function confirmRestartHost(): Promise<boolean> {
  return confirm({
    title: 'Restart Rubylight?',
    message: 'Streams in progress disconnect. This console reconnects by itself when Rubylight is back.',
    confirm: 'Restart',
  });
}

/** Leaving the page with unsaved changes asks first; switching tabs keeps the edits. */
export function installUnsavedGuard(getCount: () => number): () => void {
  const warn = (event: BeforeUnloadEvent) => event.preventDefault();
  window.addEventListener('beforeunload', warn);
  const unguard = guardNavigation((to) => {
    // Other tabs keep the edits; the Settings home is another page.
    if (to.startsWith('/settings/')) return true;
    const count = getCount();
    return confirm({
      title: 'Leave without saving?',
      message: `${count} unsaved ${count === 1 ? 'change is' : 'changes are'} lost.`,
      confirm: 'Leave',
      danger: true,
    });
  });
  return () => {
    window.removeEventListener('beforeunload', warn);
    unguard();
  };
}
