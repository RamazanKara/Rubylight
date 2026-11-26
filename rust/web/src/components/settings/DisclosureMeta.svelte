<script lang="ts">
  /**
   * The facts on a folded group of settings, as pills after its title: how many settings it
   * holds, how many differ from the default, how many wait to be saved.
   */
  let { count, customized = 0, unsaved = 0 }: { count: number; customized?: number; unsaved?: number } = $props();
</script>

<span class="pills">
  <span class="pill">{count} {count === 1 ? 'setting' : 'settings'}</span>
  {#if customized}<span class="pill customized">{customized} customized</span>{/if}
  {#if unsaved}<span class="pill unsaved">{unsaved} unsaved</span>{/if}
</span>

<style>
  .pills {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-1);
  }
  .pill {
    padding: 2px var(--space-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-pill);
    background: var(--sunken);
    color: var(--muted);
    font-size: var(--text-xs);
    font-weight: var(--weight-medium);
    font-variant-numeric: tabular-nums;
    letter-spacing: 0;
    line-height: var(--leading-snug);
    white-space: nowrap;
  }
  .customized {
    border-color: var(--accent-line);
    background: var(--accent-soft);
    color: var(--accent-text);
  }
  .unsaved {
    border-color: var(--warn-line);
    background: var(--warn-soft);
    color: var(--warn);
  }
</style>
