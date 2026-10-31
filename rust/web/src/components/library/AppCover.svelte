<script lang="ts">
  import Icon from '../Icon.svelte';
  import { api } from '../../lib/api';
  import { coverVersion, placeholder } from './app';

  let {
    name,
    uuid,
    path = '',
    src: override,
    alt = '',
  }: {
    name: string;
    uuid?: string;
    /** The app's 'image-path'; empty means no cover. */
    path?: string;
    /** Show this image instead of the host's copy (an unsaved choice). */
    src?: string;
    alt?: string;
  } = $props();

  const src = $derived(override || (uuid && path ? api.apps.coverUrl(uuid, coverVersion(path)) : ''));
  let broken = $state('');
  const blank = $derived(placeholder(name));
</script>

{#if src && broken !== src}
  <div class="cover">
    <img {src} {alt} loading="lazy" draggable="false" onerror={() => (broken = src)} />
  </div>
{:else}
  <!-- No art: a tint of its own per game, drawn like a poster so it sits well beside real covers. -->
  <div class="cover blank" style:--hue={blank.hue}>
    {#if blank.icon}
      <span class="mark icon"><Icon name={blank.icon} size={48} /></span>
    {:else}
      <span class="mark" class:long={blank.letters.length > 1} aria-hidden="true">{blank.letters}</span>
    {/if}
  </div>
{/if}

<style>
  .cover {
    container-type: inline-size;
    aspect-ratio: 2 / 3;
    width: 100%;
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--sunken);
    display: grid;
    place-items: center;
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  /* Media tokens are dark in both themes, like game art; the hue is the game's own.
     Browsers without relative colours keep the plain media background. */
  .blank {
    border-color: transparent;
    background: var(--media-bg);
    background:
      radial-gradient(130% 70% at 15% 0%, oklch(from var(--media-bg) calc(l + 0.26) 0.09 var(--hue) / 0.85), transparent 70%),
      linear-gradient(165deg, oklch(from var(--media-bg) calc(l + 0.14) 0.06 var(--hue)), oklch(from var(--media-bg) calc(l + 0.02) 0.03 var(--hue)));
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--media-ink) 10%, transparent);
    color: var(--media-ink);
  }
  .mark {
    font-family: var(--font-display);
    font-weight: var(--weight-bold);
    font-size: clamp(16px, 36cqi, 60px);
    line-height: 1;
    letter-spacing: -0.01em;
    user-select: none;
  }
  .long {
    font-size: clamp(14px, 28cqi, 48px);
  }
  .icon {
    display: grid;
    width: 38cqi;
    color: var(--media-ink-2);
  }
  .icon :global(svg) {
    width: 100%;
    height: auto;
  }
</style>
