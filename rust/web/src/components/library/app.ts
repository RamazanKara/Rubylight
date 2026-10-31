// Helpers shared by the library grid and the app editor.
import { api, type App, type SessionStatus } from '../../lib/api';
import { confirm, failed, notify } from '../../lib/feedback.svelte';

/** "Half-Life 2" → "HL": the cover placeholder. */
export function initials(name: string): string {
  const words = name.trim().split(/[\s\-_:]+/).filter(Boolean);
  return words
    .slice(0, 2)
    .map((word) => Array.from(word)[0] ?? '')
    .join('')
    .toUpperCase();
}

/** Hues (OKLCH degrees) a cover without art is tinted with: red, amber, green, teal, cyan, blue, violet, magenta. */
const COVER_HUES = [25, 70, 145, 180, 215, 255, 295, 335];

/**
 * How a cover without art looks: a tint picked from the name, so the same game
 * keeps its colour; the Desktop and Big Picture get a picture instead of letters.
 */
export function placeholder(name: string): { hue: number; letters: string; icon: 'monitor' | 'gamepad' | null } {
  const key = name.trim().toLowerCase();
  let hash = 0;
  for (const char of key) hash = (hash * 31 + (char.codePointAt(0) ?? 0)) >>> 0;
  return {
    hue: COVER_HUES[hash % COVER_HUES.length] ?? COVER_HUES[0]!,
    letters: initials(name),
    icon: key === 'desktop' ? 'monitor' : key.includes('big picture') ? 'gamepad' : null,
  };
}

/** A short hash of the cover path, so a new cover is not served from the cache. */
export function coverVersion(path: string): string {
  let hash = 5381;
  for (let index = 0; index < path.length; index++) hash = ((hash * 33) ^ path.charCodeAt(index)) >>> 0;
  return hash.toString(36);
}

export function coverPath(app: App): string {
  const path = app['image-path'];
  return typeof path === 'string' ? path : '';
}

/** The running app's name, or '' when nothing runs. */
export function runningName(status: SessionStatus | null): string {
  if (!status?.appRunning) return '';
  return status.app?.name || status.appName || '';
}

export async function launchApp(app: App): Promise<boolean> {
  if (!app.uuid) return false;
  try {
    await api.apps.launch(app.uuid);
    notify(`Started ${app.name}.`, 'ok');
    return true;
  } catch (error) {
    failed('Launching the app failed', error);
    return false;
  }
}

export function confirmClose(name: string): Promise<boolean> {
  return confirm({
    title: `Close ${name}?`,
    message: 'The app quits on this PC and every stream ends. Unsaved progress in the app may be lost.',
    confirm: 'Close app',
    danger: true,
  });
}

/** Ask with confirmClose() first. */
export async function closeApp(name: string): Promise<boolean> {
  try {
    await api.apps.close();
    notify(`Closed ${name}.`, 'ok');
    return true;
  } catch (error) {
    failed('Closing the app failed', error);
    return false;
  }
}
