import type { IconName } from '../components/Icon.svelte';
import { settingGroups } from './settings-schema';

/** A page of the console: in the sidebar, and found by the quick switcher (Ctrl+K). */
export interface NavItem {
  path: string;
  label: string;
  icon: IconName;
  /** When the sidebar marks it as the current page; by default its path and the pages under it. */
  match?: (path: string) => boolean;
  /** Other words the quick switcher finds it by. */
  keywords?: string;
}

/** The everyday pages; Help last. */
export const primary: NavItem[] = [
  { path: '/', label: 'Home', icon: 'overview', keywords: 'overview status streaming now' },
  { path: '/library', label: 'Games', icon: 'library', keywords: 'library apps covers' },
  { path: '/devices', label: 'Devices', icon: 'devices', keywords: 'clients phones tablets tv paired' },
  { path: '/settings', label: 'Settings', icon: 'settings', match: (path) => path === '/settings', keywords: 'preferences options' },
  { path: '/help', label: 'Help', icon: 'help', keywords: 'connect address log analysis findings support bundle problem' },
];

/** Everything else: folded in the sidebar under Advanced. */
export const advanced: NavItem[] = [
  {
    path: `/settings/${settingGroups[0]?.id ?? 'general'}`,
    label: 'All settings',
    icon: 'settings',
    match: (path) => path.startsWith('/settings/'),
    keywords: 'every setting configuration',
  },
  {
    path: '/troubleshoot',
    label: 'Troubleshoot',
    icon: 'maintenance',
    keywords: 'maintenance quit displays layout virtual windows session remote desktop crash co-op nucleus',
  },
  { path: '/logs', label: 'Logs', icon: 'logs', keywords: 'log file' },
  { path: '/stream', label: 'Stream in the browser', icon: 'play', keywords: 'webrtc browser play' },
  { path: '/tokens', label: 'API tokens', icon: 'key', keywords: 'api token automation' },
  { path: '/webhooks', label: 'Webhooks', icon: 'link', keywords: 'webhook events automation' },
];

/** Whether the sidebar marks this item as the page shown. */
export function isCurrent(item: NavItem, path: string): boolean {
  if (item.match) return item.match(path);
  return item.path === '/' ? path === '/' : path === item.path || path.startsWith(`${item.path}/`);
}
