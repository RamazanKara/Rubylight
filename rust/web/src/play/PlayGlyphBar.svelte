<script lang="ts">
  /**
   * The play page's glyph bar: what the input in the player's hands does here, drawn as that
   * input's own buttons (play-input-source.ts). Touch and mouse get no bar at all.
   *
   * It sits over the picture while a game plays, so it is opaque and never blurs what is behind.
   */
  import { hintText, hints, type HintContext, type InputSource } from './play-input-source';

  let { source, context, floating = false }: { source: InputSource; context: HintContext; floating?: boolean } = $props();

  const shown = $derived(hints(source, context));
  /** Face buttons draw round, as on the pad; keys and a remote's buttons as key caps. */
  const round = $derived(source === 'xbox' || source === 'playstation' || source === 'nintendo');
</script>

{#if shown.length}
  <ul class="glyphs" class:floating aria-label="Controls">
    {#each shown as hint (hint.glyph + hint.label)}
      <li>
        <span class="visually-hidden">{hintText(hint)}</span>
        {#if hint.hold}<span class="hold" aria-hidden="true">Hold</span>{/if}
        <kbd class:round={round && hint.glyph.length === 1} aria-hidden="true">{hint.glyph}</kbd>
        <span aria-hidden="true">{hint.label}</span>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .glyphs {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4em 1.2em;
    margin: 0;
    padding: 0;
    list-style: none;
    color: var(--media-ink-2);
    font-size: 0.8em;
    font-weight: var(--weight-medium);
  }
  /* Over the picture: a solid deck, no blur. */
  .floating {
    position: absolute;
    left: 50%;
    bottom: 5vh;
    z-index: var(--z-raised);
    transform: translateX(-50%);
    max-width: 90vw;
    padding: 0.5em 1em;
    border: 1px solid var(--media-line);
    border-radius: var(--radius-pill);
    background: var(--media-panel);
    box-shadow: var(--media-shadow);
    pointer-events: none;
  }
  li {
    display: flex;
    align-items: center;
    gap: 0.45em;
    white-space: nowrap;
  }
  .hold {
    color: var(--media-idle);
  }
  kbd {
    display: inline-grid;
    place-items: center;
    min-width: 1.9em;
    height: 1.9em;
    box-sizing: border-box;
    padding: 0 0.5em;
    border: 1px solid var(--media-line-strong);
    border-radius: var(--radius-sm);
    background: var(--media-panel-raised);
    box-shadow: var(--media-edge);
    color: var(--media-ink);
    font: inherit;
    font-size: 0.85em;
    font-weight: var(--weight-bold);
    line-height: 1;
  }
  kbd.round {
    padding: 0;
    border-radius: var(--radius-pill);
  }
</style>
