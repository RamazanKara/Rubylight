<script lang="ts">
  import Button from '../Button.svelte';

  let {
    pending,
    invalid,
    saving,
    saved,
    restarting,
    onreview,
    ondiscard,
    onsave,
    onrestart,
    ondismiss,
  }: {
    /** Unsaved changes, invalid drafts included. */
    pending: number;
    invalid: number;
    saving: boolean;
    /** Saved, and some of it applies after a restart. */
    saved: boolean;
    restarting: boolean;
    /** Omitted where every change is already on screen. */
    onreview?: () => void;
    ondiscard: () => void;
    onsave: () => void;
    onrestart: () => void;
    ondismiss: () => void;
  } = $props();
</script>

{#if pending > 0 || saved}
  <div class="save-bar">
    {#if pending > 0}
      <p class="status" role="status">
        <span class="dot" aria-hidden="true"></span>
        <span>{pending} unsaved {pending === 1 ? 'change' : 'changes'}</span>
        {#if invalid}<span class="problem">{invalid} {invalid === 1 ? 'setting needs' : 'settings need'} correction before saving.</span>{/if}
      </p>
      <div class="actions">
        {#if onreview}
          <Button variant="ghost" onclick={onreview}>{invalid ? 'Review errors' : 'Review changes'}</Button>
        {/if}
        <Button disabled={saving} onclick={ondiscard}>Discard</Button>
        <Button variant="primary" busy={saving} disabled={invalid > 0} onclick={onsave}>Save changes</Button>
      </div>
    {:else}
      <p class="status" role="status">Saved. Some changes apply after the host restarts.</p>
      <div class="actions">
        <Button icon="refresh" busy={restarting} onclick={onrestart}>Restart now</Button>
        <Button variant="ghost" icon="x" title="Dismiss" onclick={ondismiss}>
          <span class="visually-hidden">Dismiss</span>
        </Button>
      </div>
    {/if}
  </div>
{/if}

<style>
  .save-bar {
    position: sticky;
    bottom: var(--space-3);
    z-index: 5;
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: var(--space-2) var(--space-4);
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-lg);
    background: var(--panel-raised);
    box-shadow: var(--shadow-pop);
  }
  .status {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-1) var(--space-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
  }
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
  }
  .problem {
    color: var(--danger);
    font-weight: var(--weight-regular);
  }
  .actions {
    display: flex;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
</style>
