<script lang="ts">
  /**
   * The play dock: the stream's controls on one deck, hidden while the game plays. Holding Back,
   * Escape or the pad's left centre button for a second opens it, and so does the handle at the
   * top edge.
   *
   * It floats over live video, so its surfaces are solid media tokens: no `backdrop-filter`, which
   * stalls the compositor of TV browsers. Every control is 44 px or taller and carries `data-nav`,
   * so a remote, a pad and the arrow keys move over it (Play.svelte's `move`).
   */
  import Icon, { type IconName } from '../components/Icon.svelte';
  import PlayGlyphBar from './PlayGlyphBar.svelte';
  import type { InputSource } from './play-input-source';

  let {
    title,
    picture,
    muted,
    stats,
    fullscreen,
    canFullscreen,
    source,
    box = $bindable(),
    onresume,
    onfullscreen,
    onmute,
    onstats,
    ondisconnect,
    onquit,
  }: {
    /** The game that plays. */
    title: string;
    /** The picture in words: "HEVC · 1080p60". */
    picture: string;
    muted: boolean;
    /** Whether the stats capsule shows. */
    stats: boolean;
    fullscreen: boolean;
    /** False where the browser has no full screen (an iPhone). */
    canFullscreen: boolean;
    /** The input in the player's hands, for the hints under the controls. */
    source: InputSource;
    /** The dock's element, for the page's focus moves. */
    box?: HTMLElement;
    onresume: () => void;
    onfullscreen: () => void;
    onmute: () => void;
    onstats: () => void;
    ondisconnect: () => void;
    onquit: () => void;
  } = $props();
</script>

{#snippet control(icon: IconName, label: string, detail: string, action: () => void, pressed: boolean | undefined = undefined)}
  <button class="control" data-nav aria-pressed={pressed} onclick={action}>
    <Icon name={icon} />
    <span class="label">{label}</span>
    {#if detail}<span class="detail">{detail}</span>{/if}
  </button>
{/snippet}

<div class="scrim" bind:this={box} role="dialog" aria-modal="true" aria-label="Stream menu">
  <!-- A click beside the dock goes back to the game, as Back does. -->
  <button class="away" tabindex="-1" aria-hidden="true" onclick={onresume}></button>
  <div class="dock">
    <header>
      <div class="what">
        <p class="eyebrow">Playing</p>
        <h2>{title || 'Game'}</h2>
      </div>
      {#if picture}<p class="picture">{picture}</p>{/if}
    </header>
    <div class="controls">
      <button class="control primary" data-nav onclick={onresume}>
        <Icon name="play" />
        <span class="label">Back to game</span>
      </button>
      {#if canFullscreen}
        {@render control('fullscreen', 'Full screen', fullscreen ? 'On' : 'Off', onfullscreen, fullscreen)}
      {/if}
      {@render control(muted ? 'mute' : 'sound', 'Sound', muted ? 'Off' : 'On', onmute, !muted)}
      {@render control('stats', 'Stats', stats ? 'On' : 'Off', onstats, stats)}
    </div>
    <div class="controls leave">
      <button class="control" data-nav onclick={ondisconnect}>
        <Icon name="signout" />
        <span class="label">Disconnect</span>
        <span class="detail">The game keeps running</span>
      </button>
      <button class="control danger" data-nav onclick={onquit}>
        <Icon name="power" />
        <span class="label">Quit game</span>
        <span class="detail">Closes it on the PC</span>
      </button>
    </div>
    <PlayGlyphBar {source} context="dock" />
  </div>
</div>

<style>
  /* Dims the picture without blurring it. */
  .scrim {
    position: absolute;
    inset: 0;
    z-index: var(--z-overlay);
    display: grid;
    align-items: end;
    justify-items: center;
    padding: 5vh 5vw;
    box-sizing: border-box;
    background: var(--media-scrim);
    overflow: auto;
  }
  .away {
    position: absolute;
    inset: 0;
    border: 0;
    background: none;
    cursor: default;
  }
  .dock {
    position: relative;
    display: grid;
    gap: 1em;
    width: min(46em, 100%);
    box-sizing: border-box;
    padding: 1.2em;
    border: 1px solid var(--media-line-strong);
    border-radius: var(--radius-xl);
    background: var(--media-panel);
    box-shadow: var(--media-edge), var(--media-shadow);
    color: var(--media-ink);
    animation: rise var(--duration-base) var(--ease-standard);
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(1em);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .dock {
      animation: none;
    }
  }
  header {
    display: flex;
    flex-wrap: wrap;
    align-items: end;
    justify-content: space-between;
    gap: 0.4em 1em;
    min-width: 0;
  }
  .what {
    min-width: 0;
  }
  .eyebrow {
    margin: 0;
    color: var(--media-accent-text);
    font-size: 0.65em;
    font-weight: var(--weight-semibold);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }
  h2 {
    margin: 0;
    overflow: hidden;
    font-family: var(--font-display);
    font-size: 1.4em;
    font-weight: var(--weight-bold);
    letter-spacing: var(--tracking-display);
    line-height: var(--leading-tight);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .picture {
    margin: 0;
    padding: 0.25em 0.7em;
    border: 1px solid var(--media-line);
    border-radius: var(--radius-pill);
    background: var(--media-panel-raised);
    color: var(--media-ink-2);
    font-size: 0.7em;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  /* One row of equal controls; two to a row on a tablet, one on a phone, and never one left over. */
  .controls {
    display: grid;
    grid-auto-columns: minmax(0, 1fr);
    grid-auto-flow: column;
    gap: 0.6em;
  }
  @media (max-width: 860px) {
    .controls {
      grid-template-columns: repeat(2, minmax(0, 1fr));
      grid-auto-flow: row;
    }
    .controls > :last-child:nth-child(odd) {
      grid-column: span 2;
    }
  }
  @media (max-width: 480px) {
    .controls {
      grid-template-columns: minmax(0, 1fr);
    }
    .controls > :last-child:nth-child(odd) {
      grid-column: auto;
    }
  }
  .leave {
    padding-top: 1em;
    border-top: 1px solid var(--media-line);
  }
  .control {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 0 0.6em;
    min-height: max(44px, 3.2em);
    padding: 0.5em 0.9em;
    border: 2px solid var(--media-line);
    border-radius: var(--radius);
    background: var(--media-panel-raised);
    box-shadow: var(--media-edge);
    color: var(--media-ink);
    font: inherit;
    font-size: 0.85em;
    font-weight: var(--weight-semibold);
    text-align: start;
    cursor: pointer;
    transition:
      border-color var(--duration-fast) var(--ease-standard),
      transform var(--duration-fast) var(--ease-standard);
  }
  .control :global(svg) {
    grid-row: span 2;
    width: 1.3em;
    height: 1.3em;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .label:last-child {
    grid-row: span 2;
  }
  .detail {
    grid-column: 2;
    color: var(--media-idle);
    font-size: 0.8em;
    font-weight: var(--weight-regular);
  }
  .control:hover {
    border-color: var(--media-line-strong);
  }
  .control:focus-visible,
  .control:focus {
    outline: 4px solid var(--media-focus);
    outline-offset: 3px;
    border-color: var(--media-focus);
    transform: scale(1.03);
  }
  .primary {
    border-color: var(--media-accent);
    background: var(--media-accent);
    color: var(--media-accent-ink);
  }
  .primary:hover {
    border-color: var(--media-accent-strong);
    background: var(--media-accent-strong);
  }
  .danger {
    border-color: color-mix(in srgb, var(--media-danger) 40%, var(--media-panel));
    background: color-mix(in srgb, var(--media-danger) 14%, var(--media-panel));
    color: var(--media-danger);
  }
  .danger .detail {
    color: var(--media-ink-2);
  }
  @media (prefers-reduced-motion: reduce) {
    .control {
      transition: none;
    }
    .control:focus {
      transform: none;
    }
  }
</style>
