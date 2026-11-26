<script lang="ts">
  /**
   * The live command stage: what streams now, how it is doing and what to do about it, on one
   * cover-lit media plane. Home and Sessions show it whenever a device streams. It leads with the
   * first stream's health, traces and pipeline; every stream has its row below, with its own
   * Disconnect and its full figures.
   */
  import type { Snippet } from 'svelte';
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import AppCover from '../library/AppCover.svelte';
  import { coverPath, coverVersion, placeholder } from '../library/app';
  import PipelineTimeline from './PipelineTimeline.svelte';
  import StreamCard from './StreamCard.svelte';
  import TelemetrySparklines from './TelemetrySparklines.svelte';
  import { diagnosticsText, leadStream, pipelinePhases, specPills, stageTitle, traces, worstHealth } from './studio-telemetry';
  import { api, type App, type SessionStatus, type StreamSession } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { link } from '../../lib/router.svelte';

  interface Props {
    /** The streams to devices, the first one leading. Never empty. */
    streams: StreamSession[];
    /** The game that plays, when the host knows it. */
    game?: { name: string; app?: App | null } | null;
    /** The running app, for Quit game; without it the stage offers no quit. */
    status?: SessionStatus | null;
    /** The last poll went unanswered: the figures stay, marked as old. */
    stale?: boolean;
    /** The region's name. */
    label?: string;
    /** uuid of the device being disconnected. */
    disconnecting?: string | null;
    ondisconnect: (stream: StreamSession) => void;
    /** After the game was quit. */
    onchange?: () => void;
    /** The stage's last row: what the page adds under the streams. */
    children?: Snippet;
  }
  let { streams, game = null, status = null, stale = false, label = 'Live stream', disconnecting = null, ondisconnect, onchange, children }: Props = $props();

  const lead = $derived(leadStream(streams) as StreamSession);
  const health = $derived(worstHealth(streams));
  const measured = $derived(lead.performance.sample_frames > 0);
  const phases = $derived(pipelinePhases(lead.performance.stages, measured));
  const pills = $derived(specPills(lead));
  const title = $derived(stageTitle(streams, game?.name ?? null));
  const cover = $derived.by(() => {
    if (!game) return null;
    const path = game.app ? coverPath(game.app) : '';
    const uuid = game.app?.uuid;
    return { image: uuid && path ? api.apps.coverUrl(uuid, coverVersion(path)) : '', hue: placeholder(game.name).hue, path, uuid };
  });
  const appName = $derived(status?.app?.name || status?.appName || game?.name || 'the game');

  let rows = $state<HTMLElement>();
  let quitting = $state(false);

  /** Opens the first stream's figures and brings them into view. */
  function showDetails() {
    const details = rows?.querySelector('details');
    if (!details) return;
    details.open = true;
    details.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    details.querySelector('summary')?.focus({ preventScroll: true });
  }

  async function copyDiagnostics() {
    const text = streams.map((stream) => diagnosticsText(stream, game?.name ?? null)).join('\n\n');
    try {
      await navigator.clipboard.writeText(text);
      notify('Copied the stream’s figures.', 'ok');
    } catch (failure) {
      failed('Copying the figures failed', failure);
    }
  }

  async function quit() {
    const app = appName;
    const ok = await confirm({
      title: `Quit ${app}?`,
      message: 'It closes on this PC and every stream ends. Unsaved progress may be lost.',
      confirm: 'Quit game',
      danger: true,
    });
    if (!ok) return;
    quitting = true;
    try {
      await api.apps.close();
      notify(`Closed ${app}.`, 'ok');
      onchange?.();
    } catch (failure) {
      failed('Quitting the game failed', failure);
    } finally {
      quitting = false;
    }
  }
</script>

<section class="stage" aria-label={label}>
  <div class="backdrop" class:tint={cover && !cover.image} style:--hue={cover?.hue ?? 350}
    style:background-image={cover?.image ? `url("${cover.image}")` : undefined} aria-hidden="true"></div>
  <div class="main" class:bare={!(game && cover)}>
    <div class="top">
      {#if health}
        <p class="health {health.level}" role="status">
          <span class="dot" aria-hidden="true"></span>
          <strong>{health.label}</strong>
          <span class="cause">{health.cause}</span>
        </p>
      {/if}
      {#if stale}<p class="old" role="status"><Icon name="alert" size={16} />Waiting for this PC: these figures are a few seconds old.</p>{/if}
    </div>
    <div class="what">
      <p class="eyebrow">Live now</p>
      <h2>{title}</h2>
      <ul class="pills" aria-label="Stream format">
        {#each pills as pill (pill)}<li class="num">{pill}</li>{/each}
      </ul>
    </div>
    {#if game && cover}
      <a class="cover" href={cover.uuid ? `/library/${encodeURIComponent(cover.uuid)}` : '/library'} use:link aria-label="{game.name}: game page">
        <AppCover name={game.name} uuid={cover.uuid} path={cover.path} />
      </a>
    {/if}
  </div>

  <div class="figures">
    <TelemetrySparklines traces={traces(lead)} {stale} />
    <PipelineTimeline {phases} />
  </div>

  <div class="deck">
    <Button icon="info" onclick={showDetails}>Stream details</Button>
    <Button icon="copy" onclick={copyDiagnostics}>Copy diagnostics</Button>
    {#if status?.appRunning}
      <span class="end"><Button variant="danger" icon="power" busy={quitting} onclick={quit}>Quit game</Button></span>
    {/if}
  </div>

  <div class="rows" bind:this={rows}>
    {#each streams as stream (stream.uuid)}
      <StreamCard {stream} busy={disconnecting === stream.uuid} {ondisconnect} />
    {/each}
  </div>
  {@render children?.()}
</section>

<style>
  /* A media plane: dark in both themes. The theme's own tokens are pointed at the media palette
     here, so buttons, badges and the stream rows inside draw for this plane without a variant. */
  .stage {
    --panel: var(--media-panel-raised);
    --sunken: var(--media-panel);
    --field: var(--media-panel);
    --ink: var(--media-ink);
    --ink-2: var(--media-ink-2);
    --muted: var(--media-idle);
    --line: var(--media-line);
    --line-strong: var(--media-line-strong);
    --focus: var(--media-focus);
    --edge: var(--media-edge);
    --ok: var(--media-ok);
    --ok-soft: color-mix(in srgb, var(--media-ok) 14%, var(--media-panel));
    --ok-line: color-mix(in srgb, var(--media-ok) 36%, var(--media-panel));
    --warn: var(--media-warn);
    --warn-soft: color-mix(in srgb, var(--media-warn) 14%, var(--media-panel));
    --warn-line: color-mix(in srgb, var(--media-warn) 36%, var(--media-panel));
    --danger: var(--media-danger);
    --danger-soft: color-mix(in srgb, var(--media-danger) 14%, var(--media-panel));
    --danger-line: color-mix(in srgb, var(--media-danger) 40%, var(--media-panel));
    --accent-text: var(--media-accent-text);
    position: relative;
    isolation: isolate;
    display: grid;
    overflow: hidden;
    border: 1px solid var(--media-line);
    border-radius: var(--radius-xl);
    background: var(--media-bg);
    box-shadow: var(--media-edge), var(--shadow);
    color: var(--media-ink);
  }
  /* The game's cover as light: blurred to its colours, held under the wash so text keeps its contrast. */
  .backdrop {
    position: absolute;
    inset: -48px;
    z-index: var(--z-below);
    background-color: var(--media-bg);
    background-image: var(--media-glow);
    background-size: cover;
    background-position: center;
    filter: blur(36px) saturate(1.25);
    opacity: 0.85;
  }
  .backdrop.tint {
    background-image:
      radial-gradient(130% 90% at 85% 10%, oklch(from var(--media-bg) calc(l + 0.26) 0.09 var(--hue) / 0.85), transparent 70%),
      linear-gradient(165deg, oklch(from var(--media-bg) calc(l + 0.14) 0.06 var(--hue)), oklch(from var(--media-bg) calc(l + 0.02) 0.03 var(--hue)));
  }
  .stage::before {
    content: '';
    position: absolute;
    inset: 0;
    z-index: var(--z-below);
    background: var(--media-wash);
  }
  .main,
  .figures,
  .deck {
    padding-inline: var(--space-6);
  }
  /* Health sits level with the cover's top, the title with its foot: no empty band between them. */
  .top {
    grid-area: top;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    align-self: start;
    gap: var(--space-2) var(--space-4);
  }
  /* Stream health: a word in its colour and one sentence. */
  .health {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-1) var(--space-2);
    min-width: 0;
    padding: var(--space-2) var(--space-4) var(--space-2) var(--space-3);
    border: 1px solid var(--media-line);
    border-radius: var(--radius-pill);
    background: var(--media-scrim);
    font-size: var(--text-sm);
    line-height: var(--leading-snug);
  }
  .health .dot {
    width: 10px;
    height: 10px;
    flex: none;
    border-radius: 50%;
    background: var(--tone);
    box-shadow: var(--halo);
  }
  .health strong {
    color: var(--tone);
    font-weight: var(--weight-semibold);
  }
  .health .cause {
    color: var(--media-ink-2);
  }
  .healthy {
    --tone: var(--media-ok);
    --halo: var(--media-halo-ok);
  }
  .attention {
    --tone: var(--media-warn);
    --halo: var(--media-halo-warn);
  }
  .action {
    --tone: var(--media-danger);
    --halo: var(--media-halo-danger);
    border-color: var(--danger-line);
  }
  .healthy .dot {
    animation: beat var(--pulse-duration) ease-in-out infinite;
  }
  .old {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--media-warn);
    font-size: var(--text-sm);
  }
  .main {
    display: grid;
    grid-template: 'top cover' auto 'what cover' 1fr / minmax(0, 1fr) auto;
    gap: var(--space-5) var(--space-6);
    padding-block: var(--space-5);
  }
  .main.bare {
    grid-template: 'top' auto 'what' auto / minmax(0, 1fr);
  }
  .what {
    grid-area: what;
    display: grid;
    align-self: end;
    gap: var(--space-2);
    min-width: 0;
  }
  .eyebrow {
    color: var(--media-ink-2);
  }
  h2 {
    max-width: 22ch;
    font-size: clamp(var(--text-xl), 3.2vw, var(--text-2xl));
    letter-spacing: var(--tracking-display);
  }
  .pills {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin: var(--space-2) 0 0;
    padding: 0;
    list-style: none;
  }
  .pills li {
    padding: 3px var(--space-3);
    border: 1px solid var(--media-line-strong);
    border-radius: var(--radius-pill);
    background: var(--media-scrim);
    color: var(--media-ink);
    font-size: var(--text-xs);
    font-weight: var(--weight-medium);
    line-height: var(--leading);
    white-space: nowrap;
  }
  .cover {
    grid-area: cover;
    display: block;
    align-self: end;
    width: 132px;
    border-radius: var(--radius);
    box-shadow: var(--media-shadow);
    transition: transform var(--duration-fast) var(--ease-standard);
  }
  .cover:hover {
    transform: translateY(-2px);
  }
  .figures {
    display: grid;
    gap: var(--space-4);
    padding-block: 0 var(--space-5);
  }
  /* The action deck: calm, each button says what it does. Quit, the one that ends the game, stands apart. */
  .deck {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    padding-block: var(--space-3);
    border-top: 1px solid var(--media-line);
    background: var(--media-scrim);
  }
  .end {
    display: inline-flex;
    margin-inline-start: auto;
  }
  .rows {
    display: grid;
    border-top: 1px solid var(--media-line);
    background: var(--media-panel);
  }
  .rows > :global(* + *) {
    border-top: 1px solid var(--media-line);
  }
  @keyframes beat {
    50% {
      opacity: 0.45;
    }
  }
  @media (max-width: 700px) {
    .main,
    .figures,
    .deck {
      padding-inline: var(--space-4);
    }
    .health {
      border-radius: var(--radius);
    }
    .main {
      grid-template: 'top top' auto 'what cover' auto / minmax(0, 1fr) 72px;
      gap: var(--space-4);
      padding-block: var(--space-4);
    }
    .what {
      align-self: start;
    }
    .cover {
      align-self: start;
      width: 72px;
    }
    .deck :global(.button) {
      flex: 1 1 auto;
    }
    .end {
      flex: 1 1 100%;
      margin-inline-start: 0;
    }
    .end :global(.button) {
      width: 100%;
    }
  }
</style>
