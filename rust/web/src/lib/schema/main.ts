// What the Settings home shows, and how All settings folds the rest
// (docs: unified/ux/direction.md 2.2-2.3). The keys are the schema's own;
// this file only chooses and orders them.
import type { ConfigValue } from '../api';
import type { Option, Setting } from '../settings-types';

export interface MainSection {
  id: string;
  title: string;
  description: string;
  /** Schema keys in the order they show. */
  keys: readonly string[];
}

/**
 * The Settings home: what a typical player changes in week one, phrased as
 * an outcome, with every option safe. At most 12 keys in all.
 */
export const MAIN_SECTIONS: readonly MainSection[] = [
  {
    id: 'pc',
    title: 'This PC',
    description: 'How devices find this PC and whether new ones may pair.',
    keys: ['sunshine_name', 'enable_pairing'],
  },
  {
    id: 'picture',
    title: 'Picture',
    description: 'Stream quality and what happens to your screens while you play.',
    keys: ['stream_preset', 'virtual_display_mode', 'virtual_display_layout'],
  },
  {
    id: 'sound',
    title: 'Sound and controls',
    description: 'What your devices can hear and use on this PC.',
    keys: ['stream_audio', 'stream_mic', 'controller'],
  },
  {
    id: 'games',
    title: 'Games',
    description: 'What happens to a game when you stop playing.',
    keys: ['terminate_on_pause'],
  },
  {
    id: 'updates',
    title: 'Updates',
    description: 'New Rubylight releases.',
    keys: ['auto_update'],
  },
];

export const MAIN_KEYS: readonly string[] = MAIN_SECTIONS.flatMap((section) => section.keys);
export const MAIN_BUDGET = 12;

/**
 * Right for everyone, no added latency, no PC side effect, stock Moonlight
 * unaffected and verified: on with no switch of their own. Their keys stay
 * readable and show only in All settings, General, Debug switches.
 */
export const ALWAYS_ON: readonly string[] = ['qr_pairing', 'link_rate_adaptation'];
export const DEBUG_GROUP = 'Debug switches';

/** The only groups All settings starts collapsed. */
export const EXPERT_GROUPS: ReadonlySet<string> = new Set([
  'Capture troubleshooting',
  'AMD AMF',
  'RTSS',
  'Ports and addresses',
  'Console',
  DEBUG_GROUP,
]);

/** An always-on setting as All settings shows it: in Debug switches, for troubleshooting. */
export function asDebugSwitch(setting: Setting): Setting {
  return {
    ...setting,
    category: 'advanced',
    group: DEBUG_GROUP,
    description: 'On for everyone. Turning it off is only for troubleshooting.',
    details: [setting.description, setting.details].filter(Boolean).join(' '),
    pending: undefined,
  };
}

/**
 * The monitor layout as an outcome: Turn off is Exclusive, Stay on is Extend.
 * Another layout chosen in All settings stays listed under its own name, so
 * the home never rewrites it.
 */
export function monitorsWhileStreaming(setting: Setting, current: ConfigValue): Setting {
  if (setting.control.kind !== 'select') return setting;
  const options: Option[] = [
    { value: 'exclusive', label: 'Turn off' },
    { value: 'extended', label: 'Stay on' },
  ];
  const other = setting.control.options.find(
    (option) => option.value === current && !options.some((item) => item.value === option.value),
  );
  if (other) options.push({ value: other.value, label: `Stay on: ${other.label}` });
  return {
    ...setting,
    label: 'While streaming, your monitors',
    description: 'Turn off shows the game only on your device. Stay on keeps your monitors as they are, next to the stream.',
    control: { kind: 'select', options },
  };
}

/** How a main key looks on the Settings home. */
export function onHome(setting: Setting, current: ConfigValue): Setting {
  return setting.key === 'virtual_display_layout' ? monitorsWhileStreaming(setting, current) : setting;
}
