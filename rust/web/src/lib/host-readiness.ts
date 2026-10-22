import type { Metadata } from './api';

type Tone = 'neutral' | 'ok' | 'warn' | 'danger';
interface Check {
  title: string;
  tone: Tone;
  state: string;
  /** One line: what the host found. */
  text: string;
  /** What to do about a problem. */
  help?: string;
  /** The host's own text, shown as is behind Details. */
  detail?: string;
  href: string;
  settings: string;
}

const list = new Intl.ListFormat('en', { style: 'long', type: 'conjunction' });
const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

const CAPTURE: Record<string, string> = {
  'idd-push': 'Direct driver push',
  wgc: 'Windows Graphics Capture',
  dxgi: 'Desktop Duplication',
  ddx: 'Desktop Duplication',
};

function encoder(meta: Metadata): Check {
  const base = { title: 'Video encoder', href: '/settings/encoders', settings: 'Encoder settings' };
  const status = meta.encoder_status;
  const warning = meta.warnings?.find((warning) => warning.code === 'video_encoder');
  if (warning) {
    return {
      ...base, tone: 'danger', state: 'Retrying',
      text: 'The video encoder could not start. Rubylight keeps retrying.',
      help: 'Check the selected encoder and graphics adapter in Encoder settings. If they are correct, update or reinstall the graphics driver.',
      detail: warning.message,
    };
  }
  if (status.state === 'checking') {
    return { ...base, tone: 'neutral', state: 'Checking', text: 'Testing which codecs the graphics card can encode.' };
  }
  const codecs = [
    status.h264 && 'H.264',
    status.hevc && 'HEVC',
    status.av1 && 'AV1',
    status.pyrowave && 'PyroWave',
  ].filter((codec): codec is string => Boolean(codec));
  if (status.state === 'failed' || !codecs.length) {
    return {
      ...base,
      tone: 'danger',
      state: 'Unavailable',
      text: 'No video encoder available. Rubylight keeps retrying.',
      help: 'Check the selected encoder and graphics adapter in Encoder settings, then check the graphics driver.',
    };
  }
  if (!status.h264) {
    return {
      ...base,
      tone: 'warn',
      state: 'Limited',
      text: `Encodes ${list.format(codecs)}. Devices that only decode H.264 can't stream.`,
    };
  }
  return { ...base, tone: 'ok', state: 'Ready', text: `Encodes ${list.format(codecs)}.` };
}

function virtualDisplay(meta: Metadata): Check {
  const base = { title: 'Virtual display', href: '/settings/display', settings: 'Display settings' };
  const driver = meta.virtual_display;
  const on = meta.capture_status.virtual_display_configured;
  const detail = driver.reason || undefined;
  if (driver.ready) {
    return on
      ? { ...base, tone: 'ok', state: 'Ready', text: 'Streams use a virtual display.' }
      : { ...base, tone: 'neutral', state: 'Off', text: 'The driver is ready. Streams use a physical display.' };
  }
  if (driver.capable) {
    return { ...base, tone: 'warn', state: 'Not ready', text: "The driver is installed but isn't ready yet.", detail };
  }
  if (on) {
    return {
      ...base,
      tone: 'danger',
      state: 'Unavailable',
      text: "It's turned on, but the driver can't be opened.",
      help: 'Check that the Rubylight service is running, or turn off the virtual display to use a monitor.',
      detail,
    };
  }
  return { ...base, tone: 'neutral', state: 'Off', text: 'Not available. Streams use a physical display.', detail };
}

function audio(meta: Metadata): Check {
  const base = { title: 'Audio', href: '/settings/audio', settings: 'Audio settings' };
  if (!meta.audio_enabled) {
    return { ...base, tone: 'neutral', state: 'Off', text: 'Audio streaming is turned off. Streams have no sound.' };
  }
  if (meta.audio_error) {
    return { ...base, tone: 'danger', state: 'Error', text: "Audio outputs can't be read.", detail: meta.audio_error };
  }
  const sinks = meta.audio_sinks ?? [];
  if (!sinks.length) {
    return {
      ...base,
      tone: 'warn',
      state: 'No output',
      text: 'No audio output was found, so streams have no sound.',
      help: 'Connect speakers or headphones, or turn on an audio device in Windows sound settings.',
    };
  }
  const fallback = sinks.find((sink) => sink.default);
  const name = fallback ? fallback.name || fallback.description : '';
  return {
    ...base,
    tone: 'ok',
    state: 'Ready',
    text: `${plural(sinks.length, 'output', 'outputs')} found.${name ? ` Default is ${name}.` : ''}`,
  };
}

function capture(meta: Metadata): Check {
  const base = { title: 'Screen capture', href: '/settings/video', settings: 'Video settings' };
  const status = meta.capture_status;
  const backend = status.configured_backend || 'auto';
  const method = backend === 'auto'
    ? 'Uses direct driver push on Rubylight virtual displays and Windows Graphics Capture on other displays.'
    : backend === 'idd-push'
      ? `Prefers ${CAPTURE[backend]} on Rubylight virtual displays, with Windows capture as fallback.`
      : `Uses ${CAPTURE[backend] ?? 'the configured capture method'}.`;
  if (status.error) {
    return { ...base, tone: 'danger', state: 'Error', text: `${method} Displays can't be read.`, detail: status.error };
  }
  const displays = status.displays?.length ?? 0;
  if (displays) {
    return { ...base, tone: 'ok', state: 'Ready', text: `${method} ${plural(displays, 'display', 'displays')} found.` };
  }
  if (status.virtual_display_configured && meta.virtual_display.ready) {
    return { ...base, tone: 'ok', state: 'Ready', text: `${method} No monitor is on; streams use the virtual display.` };
  }
  return {
    ...base,
    tone: 'warn',
    state: 'No display',
    text: `${method} No active display was found.`,
    help: 'Turn on a monitor or set up a virtual display.',
  };
}

export function hostChecks(metadata: Metadata): Check[] {
  return [encoder(metadata), virtualDisplay(metadata), audio(metadata), capture(metadata)];
}
