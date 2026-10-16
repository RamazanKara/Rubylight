// The shape of the settings schema. The data lives in settings-schema.ts.
import type { BrowseType, ConfigValue } from './api';

export type CategoryId =
  | 'general'
  | 'network'
  | 'video'
  | 'encoders'
  | 'display'
  | 'frame-limiting'
  | 'hdr'
  | 'audio'
  | 'input'
  | 'commands'
  | 'library'
  | 'advanced';

export interface Category {
  id: CategoryId;
  label: string;
  description: string;
}

export interface Option {
  value: string;
  label: string;
}

export type Control =
  | { kind: 'toggle' }
  | { kind: 'select'; options: Option[] }
  | { kind: 'number'; min?: number; max?: number; step?: number; unit?: string }
  /** `browse` adds a picker for a program, file or folder on the host. */
  | { kind: 'text'; placeholder?: string; mono?: boolean; browse?: BrowseType }
  /** A string list, stored as a JSON array of strings. */
  | { kind: 'list'; placeholder?: string }
  /** Any JSON value, edited as text and validated. */
  | { kind: 'json' }
  /** Global preparation or state commands: [{do, undo, elevated}]. */
  | { kind: 'commands' }
  /** Commands clients may run: [{name, cmd, elevated}]. */
  | { kind: 'server-commands' }
  /** A display, chosen from the host's displays (empty = automatic). */
  | { kind: 'display' }
  /** An audio endpoint, chosen from the host's endpoints (empty = default). */
  | { kind: 'audio' }
  /** A GPU, chosen by name from the host's adapters (empty = automatic). */
  | { kind: 'adapter' };

/**
 * Other spellings the host reads as one of a select's options, so a value
 * saved by an older host, the host's own fallback console or a hand edit
 * shows as that option. `match` is how the host compares text: exactly (the
 * default); ignoring letter case and surrounding spaces ('case'); that, and
 * reading - as _ ('dash'); or by its letters and digits alone, in any case
 * ('letters'). `spellings` maps other spellings, compared the same way, to
 * the option each means.
 */
export interface Aliases {
  match?: 'exact' | 'case' | 'dash' | 'letters';
  spellings?: Readonly<Record<string, string>>;
}

export interface Setting {
  key: string;
  label: string;
  /** One or two plain sentences: what it does and when to change it. */
  description: string;
  /** The finer points (limits, interactions, measurements), behind "More". */
  details?: string;
  category: CategoryId;
  /** A heading within the category, for grouping related settings. */
  group?: string;
  control: Control;
  /** For a select: other spellings the host reads as one of its options. */
  aliases?: Aliases;
  /** The value the Rust host uses when the key is not set. */
  default: ConfigValue;
  /** Hide when not relevant, given the current (edited) values. */
  visibleWhen?: (values: Record<string, ConfigValue>) => boolean;
  /** Explain a hidden setting when search or review reveals it. */
  inactiveHint?: string;
  /** Streams fail without a value while this holds, given the current (edited) values. */
  requiredWhen?: (values: Record<string, ConfigValue>) => boolean;
  /** Takes effect only after the host restarts. */
  restart?: boolean;
  advanced?: boolean;
}

export interface Schema {
  categories: Category[];
  settings: Setting[];
}
