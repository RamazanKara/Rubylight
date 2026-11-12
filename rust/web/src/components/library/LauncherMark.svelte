<script lang="ts" module>
  import type { LibrarySource } from '../../lib/api';

  /** The launchers with a mark: every library source, and Playnite. */
  export type Marked = LibrarySource | 'playnite';
</script>

<script lang="ts">
  // Each launcher's mark, drawn simply in the console's own stroke style: a recognisable
  // shape, never a wordmark or a copy of the launcher's logo.
  let { source, size = 22 }: { source: Marked; size?: number } = $props();
</script>

<svg
  class="mark"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.8"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false"
  data-source={source}
>
  {#if source === 'steam'}
    <!-- A piston: two joints on a rod, in a round badge. -->
    <circle cx="12" cy="12" r="9.5" />
    <circle cx="15" cy="9" r="2.8" />
    <circle cx="8.6" cy="15.4" r="1.9" />
    <path d="M12.9 11.1 10 14" />
  {:else if source === 'epic'}
    <!-- A shield plaque with its folded tip. -->
    <path d="M5 3h14v13.6L12 21l-7-4.4z" />
    <path d="M8.5 16.4 12 18.5l3.5-2.1" />
  {:else if source === 'battlenet'}
    <!-- Two crossing orbits. -->
    <ellipse cx="12" cy="12" rx="9.5" ry="4.2" transform="rotate(38 12 12)" />
    <ellipse cx="12" cy="12" rx="9.5" ry="4.2" transform="rotate(-38 12 12)" />
  {:else if source === 'microsoft'}
    <!-- A sphere crossed by two bowed strokes. -->
    <circle cx="12" cy="12" r="9.5" />
    <path d="M6.6 6.7c3 1.8 7.2 6.4 10.4 11M17.4 6.7c-3 1.8-7.2 6.4-10.4 11" />
  {:else if source === 'ea'}
    <!-- A peak in a round badge. -->
    <circle cx="12" cy="12" r="9.5" />
    <path d="M7.5 15.5 12 7.5l4.5 8" />
  {:else if source === 'ubisoft'}
    <!-- A swirl closing on its centre. -->
    <circle cx="12" cy="12" r="9.5" />
    <path d="M17 12a5 5 0 1 1-5-5M14.5 12A2.5 2.5 0 1 1 12 9.5" />
  {:else if source === 'gog'}
    <!-- Three rounds in a rounded plate. -->
    <rect x="2.5" y="6.5" width="19" height="11" rx="3.5" />
    <circle cx="7.3" cy="12" r="2" />
    <circle cx="12" cy="12" r="2" />
    <circle cx="16.7" cy="12" r="2" />
  {:else if source === 'playnite'}
    <!-- A play button in a rounded tile. -->
    <rect x="3" y="3" width="18" height="18" rx="4.5" />
    <path d="M10 8.3v7.4l5.6-3.7z" />
  {:else}
    <!-- Game folders. -->
    <path d="M3 6h6l2 2h10v11H3z" />
  {/if}
</svg>

<style>
  .mark {
    display: block;
    flex: none;
  }
</style>
