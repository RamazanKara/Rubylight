<script lang="ts">
  import { groupAnchor } from '../../lib/settings-schema';
  import { scrollSectionIntoTrack } from './settings-page';

  let {
    sections,
    current,
    onjump,
  }: {
    sections: string[];
    current: string;
    onjump: (event: MouseEvent, name: string) => void;
  } = $props();

  let track = $state<HTMLElement>();

  // Keep the current section's link in view in a contents bar that scrolls sideways.
  $effect(() => {
    scrollSectionIntoTrack(track, current);
  });
</script>

<div class="track" bind:this={track}>
  {#each sections as name (name)}
    <a
      href="#{groupAnchor(name)}"
      data-section={name}
      aria-current={current === name ? 'location' : undefined}
      onclick={(event) => onjump(event, name)}
    >{name}</a>
  {/each}
</div>

<style>
  .track {
    display: flex;
    flex-wrap: wrap;
    gap: 2px var(--space-1);
  }
  .track::-webkit-scrollbar {
    display: none;
  }
  .track a {
    display: inline-flex;
    align-items: center;
    flex: none;
    min-height: var(--control-height);
    padding: 0 calc(var(--space-4) - 2px);
    border: 1px solid transparent;
    border-radius: var(--radius-pill);
    color: var(--ink-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    text-decoration: none;
    white-space: nowrap;
    transition: background var(--duration-fast), color var(--duration-fast);
  }
  .track a:hover {
    background: var(--panel);
    color: var(--ink);
  }
  .track a[aria-current='location'] {
    border-color: var(--line-strong);
    background: var(--panel);
    color: var(--ink);
  }
  @container (max-width: 720px) {
    /* The names that do not fit fade out at the edge, a sign that the row scrolls. */
    .track {
      flex-wrap: nowrap;
      overflow-x: auto;
      scrollbar-width: none;
      padding-right: var(--space-6);
      mask-image: linear-gradient(to left, transparent, var(--mask-solid) var(--space-6));
    }
  }
</style>
