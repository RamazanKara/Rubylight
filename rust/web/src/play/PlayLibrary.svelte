<script lang="ts">
  /**
   * The play page's library: the PC's games as covers, with the one in focus named large above
   * the grid and its cover lighting the page behind it.
   *
   * A remote, a pad and the arrow keys move over the tiles (`data-nav`, Play.svelte's `move`); a
   * pointer hovers. The light is a still, dimmed image, and no video plays on this screen.
   */
  import type { AppEntry } from '../lib/webrtc/client';

  let {
    apps,
    covers,
    loading,
    onstart,
  }: {
    apps: readonly AppEntry[];
    /** Covers loaded so far, by app id, as `data:` URLs. */
    covers: Readonly<Record<string, string>>;
    loading: boolean;
    onstart: (app: AppEntry) => void;
  } = $props();

  /** The tile with the focus or under the pointer; the first game until one has it. */
  let focusedId = $state('');
  const focused = $derived(apps.find((app) => app.id === focusedId) ?? apps[0]);

  /** Up to two initials for a game without a cover: "Portal 2" is "P2". */
  function initials(title: string): string {
    const words = title.trim().split(/\s+/).filter(Boolean);
    return ((words[0]?.[0] ?? '') + (words.length > 1 ? (words[words.length - 1]?.[0] ?? '') : '')).toUpperCase() || '?';
  }

  /** A hue of its own for each title, so cover-less games are told apart. */
  function hue(title: string): number {
    let sum = 0;
    for (const char of title) sum = (sum * 31 + char.codePointAt(0)!) % 360;
    return sum;
  }
</script>

<section class="library" aria-label="Games">
  {#if focused && covers[focused.id]}
    {#key focused.id}
      <img class="light" src={covers[focused.id]} alt="" aria-hidden="true" />
    {/key}
  {/if}
  <header class="now">
    {#if loading}
      <p class="state" role="status">Loading games…</p>
    {:else if !focused}
      <h1>No games yet</h1>
      <p class="state">Add them in the Rubylight console on your PC.</p>
    {:else}
      <p class="state" class:running={focused.running}>
        {#if focused.running}<span class="dot" aria-hidden="true"></span>Running on the PC{:else}Ready to play{/if}
      </p>
      <h1>{focused.title}</h1>
    {/if}
  </header>
  <ul class="grid">
    {#each apps as app (app.id)}
      <li>
        <button
          class="tile"
          data-nav
          aria-label={app.running ? `${app.title}, running` : app.title}
          onclick={() => onstart(app)}
          onfocus={() => (focusedId = app.id)}
          onpointerenter={() => (focusedId = app.id)}
        >
          <span class="cover" style:--hue={hue(app.title)}>
            {#if covers[app.id]}<img src={covers[app.id]} alt="" />{:else}<span class="initial" aria-hidden="true">{initials(app.title)}</span>{/if}
            {#if app.running}<span class="badge"><span class="dot" aria-hidden="true"></span>Running</span>{/if}
          </span>
          <span class="title" aria-hidden="true">{app.title}</span>
        </button>
      </li>
    {/each}
  </ul>
</section>

<style>
  .library {
    position: relative;
    display: grid;
    gap: 1.5em;
    min-width: 0;
    isolation: isolate;
  }
  /* The focused cover as light: a still image behind the title, dimmed towards the grid. */
  .light {
    position: absolute;
    top: -8em;
    right: -5vw;
    left: 15%;
    z-index: var(--z-below);
    width: calc(85% + 5vw);
    height: 28em;
    object-fit: cover;
    opacity: 0.4;
    filter: blur(40px) saturate(1.3);
    /* Faded on every side: a mask cuts a blur off at the box, so the box itself has no edge. */
    mask-image: radial-gradient(ellipse 50% 50% at 55% 40%, var(--media-black) 10%, transparent);
    pointer-events: none;
    animation: lit var(--duration-slow) var(--ease-standard);
  }
  @keyframes lit {
    from {
      opacity: 0;
    }
  }
  .now {
    display: grid;
    gap: 0.3em;
    min-height: 5.2em;
    align-content: end;
  }
  h1 {
    margin: 0;
    overflow: hidden;
    font-family: var(--font-display);
    font-size: 2.6em;
    font-weight: var(--weight-bold);
    letter-spacing: var(--tracking-display);
    line-height: var(--leading-tight);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .state {
    display: flex;
    align-items: center;
    gap: 0.5em;
    margin: 0;
    color: var(--media-idle);
    font-size: 0.75em;
    font-weight: var(--weight-semibold);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }
  .state.running {
    color: var(--media-ok);
  }
  .dot {
    width: 0.55em;
    height: 0.55em;
    border-radius: var(--radius-pill);
    background: var(--media-ok);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(9.5em, 1fr));
    gap: 1.2em;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .tile {
    display: grid;
    gap: 0.5em;
    width: 100%;
    padding: 0;
    border: 0;
    background: none;
    color: var(--media-ink-2);
    font: inherit;
    text-align: start;
    cursor: pointer;
    transition: transform var(--duration-fast) var(--ease-standard);
  }
  .cover {
    position: relative;
    display: grid;
    place-items: center;
    aspect-ratio: 3 / 4;
    overflow: hidden;
    border-radius: var(--radius);
    background-color: var(--media-panel);
    background-image:
      radial-gradient(130% 90% at 85% 10%, oklch(from var(--media-bg) calc(l + 0.26) 0.09 var(--hue) / 0.85), transparent 70%),
      linear-gradient(165deg, oklch(from var(--media-bg) calc(l + 0.14) 0.06 var(--hue)), oklch(from var(--media-bg) calc(l + 0.02) 0.03 var(--hue)));
    box-shadow: var(--ring-media);
    outline: 4px solid transparent;
    outline-offset: 3px;
    transition: outline-color var(--duration-fast) var(--ease-standard);
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .initial {
    color: var(--media-ink);
    font-family: var(--font-display);
    font-size: 2.2em;
    font-weight: var(--weight-bold);
    letter-spacing: var(--tracking-display);
  }
  .badge {
    position: absolute;
    top: 0.5em;
    left: 0.5em;
    display: flex;
    align-items: center;
    gap: 0.4em;
    padding: 0.2em 0.6em;
    border: 1px solid var(--media-line-strong);
    border-radius: var(--radius-pill);
    background: var(--media-panel);
    color: var(--media-ink);
    font-size: 0.65em;
    font-weight: var(--weight-semibold);
  }
  .title {
    overflow: hidden;
    font-size: 0.85em;
    font-weight: var(--weight-medium);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tile:focus {
    outline: none;
    color: var(--media-ink);
    transform: scale(1.06);
  }
  .tile:focus .cover {
    outline-color: var(--media-focus);
    box-shadow: var(--ring-media), var(--media-shadow);
  }
  .tile:hover {
    color: var(--media-ink);
  }
  @media (prefers-reduced-motion: reduce) {
    .tile {
      transition: none;
    }
    .tile:focus {
      transform: none;
    }
    .light {
      animation: none;
    }
  }
  @media (max-width: 600px) {
    h1 {
      font-size: 1.8em;
    }
    .grid {
      grid-template-columns: repeat(auto-fill, minmax(7em, 1fr));
      gap: 0.9em;
    }
  }
</style>
