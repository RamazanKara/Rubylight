// The settings the Rust host reads, with its real defaults. Each part file
// covers some categories.
import type { Category, Schema } from './settings-types';
import { settings as basics } from './schema/basics';
import { settings as video } from './schema/video';
import { settings as display } from './schema/display';
import { settings as library } from './schema/library';

const categories: Category[] = [
  { id: 'general', label: 'General', description: 'Host name, pairing, updates and logs' },
  { id: 'network', label: 'Network', description: 'Ports, encryption and console access' },
  { id: 'video', label: 'Video', description: 'Codecs, bitrate and frame pacing' },
  { id: 'encoders', label: 'Encoders', description: 'Which GPU and encoder compress the video' },
  { id: 'display', label: 'Display', description: 'Virtual displays, resolution and restore' },
  { id: 'frame-limiting', label: 'Frame limiting', description: 'Frame rate caps for games' },
  { id: 'hdr', label: 'HDR', description: 'HDR on the streamed display' },
  { id: 'audio', label: 'Audio', description: 'Sound, speakers and microphone' },
  { id: 'input', label: 'Input', description: 'Keyboard, mouse and controllers' },
  { id: 'commands', label: 'Commands', description: 'Commands run around apps or from a device' },
  { id: 'library', label: 'Library', description: 'Steam, Playnite and Lossless Scaling' },
  { id: 'advanced', label: 'Advanced', description: 'Compatibility and custom settings' },
];

export const schema: Schema = {
  categories,
  settings: [...basics, ...video, ...display, ...library],
};
