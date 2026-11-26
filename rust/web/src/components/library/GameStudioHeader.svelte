<script lang="ts">
  /**
   * The game page's header: the game's cover, lit from behind by a wash of the same art, its
   * launcher, its name, the stream profile it carries and the page's quick actions.
   *
   * A media plane, dark in both themes like game art. The wash is a blurred copy of the cover
   * (a still image; there is no video on this page), so a game without art keeps its own tint.
   */
  import type { Snippet } from 'svelte';
  import AppCover from './AppCover.svelte';
  import Badge from '../Badge.svelte';
  import type { StudioFact } from './game-studio';

  let {
    name,
    uuid,
    cover = '',
    launcher = '',
    status = null,
    facts = [],
    actions,
    aside,
  }: {
    name: string;
    uuid?: string;
    /** The saved app's 'image-path'. */
    cover?: string;
    /** The launcher that starts it, by name; empty for the user's own programs. */
    launcher?: string;
    status?: 'running' | 'paused' | null;
    facts?: readonly StudioFact[];
    /** Launch, Edit cover. */
    actions?: Snippet;
    /** The game's place in the library and the buttons that move it. */
    aside?: Snippet;
  } = $props();
</script>

<header class="studio">
  <div class="art"><AppCover {name} {uuid} path={cover} /></div>
  <div class="what">
    <p class="eyebrow">
      <span>{launcher || 'Your own app'}</span>
      {#if status === 'paused'}<Badge tone="warn">Paused</Badge>{:else if status === 'running'}<Badge tone="live">Running</Badge>{/if}
    </p>
    <h1>{name}</h1>
    {#if facts.length}
      <ul class="facts" aria-label="Stream profile">
        {#each facts as fact (fact.text)}
          <li><Badge tone={fact.tone}>{fact.text}</Badge></li>
        {/each}
      </ul>
    {/if}
    {#if actions}<div class="actions">{@render actions()}</div>{/if}
  </div>
  {#if aside}<div class="aside">{@render aside()}</div>{/if}
  <!-- After the cover in the document: the router morphs the first cover of a game it finds. -->
  <div class="wash" aria-hidden="true"><AppCover {name} path={cover} {uuid} /></div>
</header>

<style>
  .studio {
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
    --accent-text: var(--media-accent-text);
    --accent-soft: color-mix(in srgb, var(--media-accent-text) 16%, var(--media-panel));
    --accent-line: color-mix(in srgb, var(--media-accent-text) 40%, var(--media-panel));
    position: relative;
    isolation: isolate;
    display: grid;
    grid-template-columns: 132px minmax(0, 1fr) auto;
    align-items: end;
    gap: var(--space-5);
    overflow: hidden;
    padding: var(--space-6);
    border: 1px solid var(--media-line);
    border-radius: var(--radius-xl);
    background: var(--media-bg);
    box-shadow: var(--media-edge), var(--shadow-raised);
    color: var(--media-ink);
    min-width: 0;
  }
  .art {
    align-self: center;
    border-radius: var(--radius);
    box-shadow: var(--media-shadow);
  }
  /* The same art, spread and blurred behind everything, under a scrim that keeps the text readable. */
  .wash {
    position: absolute;
    inset: 0;
    z-index: var(--z-below);
    overflow: hidden;
    pointer-events: none;
  }
  .wash :global(.cover) {
    position: absolute;
    inset: -40% -10%;
    width: auto;
    height: auto;
    aspect-ratio: auto;
    border: 0;
    filter: blur(56px) saturate(1.35);
    opacity: 0.6;
    box-shadow: none;
  }
  .wash :global(.mark) {
    display: none;
  }
  .wash::after {
    content: '';
    position: absolute;
    inset: 0;
    background: linear-gradient(100deg, var(--media-scrim), transparent 85%), linear-gradient(0deg, var(--media-scrim), transparent 70%);
  }
  .what {
    display: grid;
    gap: var(--space-3);
    min-width: 0;
  }
  .eyebrow {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    color: var(--media-ink-2);
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }
  h1 {
    margin: 0;
    overflow-wrap: anywhere;
    color: var(--media-ink);
    font-size: var(--text-2xl);
    line-height: var(--leading-tight);
    letter-spacing: var(--tracking-display);
  }
  .facts {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    margin-top: var(--space-2);
  }
  .aside {
    align-self: start;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: var(--space-2);
    max-width: 280px;
  }
  @media (max-width: 900px) {
    .studio {
      grid-template-columns: 132px minmax(0, 1fr);
    }
    .aside {
      grid-column: 1 / -1;
      justify-content: flex-start;
      max-width: none;
    }
  }
  /* On a phone the cover keeps the name company, and the actions take the full width below. */
  @media (max-width: 600px) {
    .studio {
      grid-template-columns: 84px minmax(0, 1fr);
      align-items: start;
      gap: var(--space-2) var(--space-4);
      padding: var(--space-4);
    }
    .what {
      display: contents;
    }
    .art {
      grid-row: 1 / span 3;
      align-self: start;
    }
    .eyebrow,
    h1,
    .facts {
      grid-column: 2;
    }
    .actions {
      grid-column: 1 / -1;
      margin-top: var(--space-2);
    }
    h1 {
      font-size: var(--text-xl);
    }
  }
</style>
