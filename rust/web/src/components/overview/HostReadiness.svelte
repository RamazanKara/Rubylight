<script lang="ts">
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import type { Metadata } from '../../lib/api';
  import { link } from '../../lib/router.svelte';

  let {
    metadata,
    error,
    checking,
    onrecheck,
  }: { metadata: Metadata | null; error: string | null; checking: boolean; onrecheck: () => void } = $props();

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
    const detail = driver.reason || undefined;
    if (driver.ready) {
      return { ...base, tone: 'ok', state: 'Ready', text: 'Streams use a virtual display.', detail };
    }
    if (driver.capable) {
      return {
        ...base,
        tone: 'warn',
        state: 'Restart needed',
        text: 'The display driver was updated and runs only after Windows restarts.',
        help: 'Restart Windows, then start the stream again.',
        detail,
      };
    }
    return {
      ...base,
      tone: 'danger',
      state: 'Unavailable',
      text: "The display driver is missing or isn't running. Streams can't start without it.",
      help: 'Run Rubylight setup again to repair the installation.',
      detail,
    };
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

  const checks = $derived(metadata ? [encoder(metadata), virtualDisplay(metadata), audio(metadata)] : []);
  const attention = $derived(checks.filter((check) => check.tone === 'warn' || check.tone === 'danger').length);
  const summary = $derived(
    !metadata
      ? undefined
      : attention
        ? `${plural(attention, 'check needs', 'checks need')} attention.`
        : 'Ready to stream.',
  );
</script>

<Panel title="Host readiness" description={summary} flush>
  {#snippet actions()}
    <Button size="sm" variant="ghost" icon="refresh" busy={checking} onclick={onrecheck}>Recheck</Button>
  {/snippet}
  {#if error}
    <p class="notice danger error" role="alert">Host details didn't load: {error}</p>
  {/if}
  {#if !metadata}
    {#if !error}<p class="placeholder muted">Checking the host…</p>{/if}
  {:else}
    <ul>
      {#each checks as check (check.title)}
        {@const problem = check.tone === 'warn' || check.tone === 'danger'}
        <li>
          <div class="head">
            <h3>{check.title}</h3>
            <Badge tone={check.tone}>{check.state}</Badge>
          </div>
          <p class="text">{check.text}</p>
          {#if check.help}<p class="help">{check.help}</p>{/if}
          {#if check.detail}
            <!-- The host's own words: open for a problem, a click away otherwise. -->
            <details open={problem}>
              <summary>Details</summary>
              <p class="detail">{check.detail}</p>
            </details>
          {/if}
          <a href={check.href} use:link>{check.settings}</a>
        </li>
      {/each}
    </ul>
  {/if}
</Panel>

<style>
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  li {
    display: grid;
    gap: 2px;
    padding: var(--space-3) var(--space-5);
    font-size: var(--text-sm);
  }
  li + li {
    border-top: 1px solid var(--line);
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--space-2);
  }
  h3 {
    font-size: var(--text);
  }
  .text {
    color: var(--ink-2);
  }
  .help {
    color: var(--ink);
  }
  details {
    font-size: var(--text-xs);
    color: var(--muted);
  }
  summary {
    cursor: pointer;
    width: fit-content;
  }
  summary:hover {
    color: var(--ink);
  }
  .detail {
    margin-top: 2px;
    padding: var(--space-2) var(--space-3);
    border-left: 2px solid var(--line-strong);
    background: var(--sunken);
    color: var(--ink-2);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }
  a {
    justify-self: start;
    margin-top: 2px;
    color: var(--muted);
    font-size: var(--text-xs);
  }
  a:hover {
    color: var(--ink);
  }
  .placeholder {
    padding: var(--space-4) var(--space-5);
    font-size: var(--text-sm);
  }
  .error {
    margin: var(--space-3) var(--space-5);
  }
  @media (max-width: 520px) {
    li {
      padding: var(--space-3) var(--space-4);
    }
    .placeholder {
      padding: var(--space-4);
    }
    .error {
      margin: var(--space-3) var(--space-4);
    }
  }
</style>
