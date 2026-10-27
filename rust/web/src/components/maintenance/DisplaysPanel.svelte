<script lang="ts">
  import { onMount } from 'svelte';
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import LoadState from '../LoadState.svelte';
  import { api, type GoldenStatus } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { reason } from './util';

  type Action = 'save' | 'restore' | 'delete' | 'disconnect' | 'reset';

  let golden = $state<GoldenStatus | null>(null);
  let loadError = $state('');
  let running = $state<Action | null>(null);

  async function load() {
    try {
      golden = await api.displays.goldenStatus();
      loadError = '';
    } catch (error) {
      loadError = reason(error);
    }
  }

  async function run(action: Action, task: () => Promise<unknown>, done: string, failure: string) {
    running = action;
    try {
      await task();
      notify(done, 'ok');
    } catch (error) {
      failed(failure, error);
    } finally {
      running = null;
    }
    await load();
  }

  async function save() {
    if (
      golden?.exists &&
      !(await confirm({
        title: 'Replace the saved layout?',
        message: 'The current display layout replaces the one saved before.',
        confirm: 'Replace',
      }))
    )
      return;
    await run('save', api.displays.exportGolden, 'Display layout saved.', 'Saving the layout failed');
  }

  const restore = () =>
    run('restore', api.displays.restoreGolden, 'Saved layout restored.', 'Restoring the layout failed');
  async function reset() {
    const ok = await confirm({
      title: 'Reset display settings memory?',
      message: 'Rubylight forgets the display changes it would undo after a stream. Displays stay as they are now.',
      confirm: 'Reset',
      danger: true,
    });
    if (ok)
      await run('reset', api.displays.resetPersistence, 'Display settings memory reset.', 'Resetting display settings failed');
  }

  async function remove() {
    const ok = await confirm({
      title: 'Delete the saved layout?',
      message: 'Rubylight stops restoring a layout after streams until you save one again.',
      confirm: 'Delete',
      danger: true,
    });
    if (ok) await run('delete', api.displays.deleteGolden, 'Saved layout deleted.', 'Deleting the layout failed');
  }

  async function disconnect() {
    const ok = await confirm({
      title: 'Disconnect virtual displays?',
      message: 'This stops every stream and removes the virtual displays Rubylight created.',
      confirm: 'Disconnect',
      danger: true,
    });
    if (ok)
      await run('disconnect', api.displays.disconnectVirtual, 'Virtual displays disconnected.', 'Disconnecting failed');
  }

  onMount(load);
</script>

<Panel title="Displays" level={3} description="After each stream, Rubylight restores the saved layout if all of its displays are connected.">
  {#snippet actions()}
    {#if golden?.exists}
      {#if golden.comparison_available && golden.current_mismatch_reason}
        <Badge tone="warn">Differs from current</Badge>
      {:else if golden.comparison_available}
        <Badge tone="ok">Matches current</Badge>
      {:else}
        <Badge>Saved</Badge>
      {/if}
    {:else if golden}
      <Badge>No saved layout</Badge>
    {/if}
  {/snippet}
  {#if golden}
    <div class="stack body">
      {#if loadError}
        <LoadState error={loadError} what="the display layout" stale onretry={load} />
      {/if}
      <div class="columns">
        <div class="group">
          <h4>Saved layout</h4>
          <p class="text">
            {#if !golden.exists}
              No layout is saved. Arrange your displays the way you want them between streams, then save.
            {:else if !golden.comparison_available}
              A layout is saved. The host could not compare it with the current displays.
            {:else if golden.current_mismatch_reason}
              A layout is saved. The displays are arranged differently right now; restore it to go back.
            {:else}
              A layout is saved and matches the displays as they are now.
            {/if}
          </p>
          <div class="row">
            <Button disabled={running !== null} busy={running === 'save'} onclick={save}>Save current layout</Button>
            <Button disabled={!golden.exists || running !== null} busy={running === 'restore'} onclick={restore}>
              Restore saved layout
            </Button>
            <Button variant="danger" disabled={!golden.exists || running !== null} busy={running === 'delete'} onclick={remove}>
              Delete saved layout
            </Button>
          </div>
        </div>
        <div class="group recovery">
          <h4>Recovery</h4>
          <div class="action">
            <div>
              <p class="text">Disconnect virtual displays</p>
              <p class="muted hint">Stops every stream and removes the virtual displays Rubylight created.</p>
            </div>
            <Button variant="danger" disabled={running !== null} busy={running === 'disconnect'} onclick={disconnect}>
              Disconnect
            </Button>
          </div>
          <div class="action">
            <div>
              <p class="text">Reset display settings memory</p>
              <p class="muted hint">Use it when displays keep returning to an old setup after streams.</p>
            </div>
            <Button variant="danger" disabled={running !== null} busy={running === 'reset'} onclick={reset}>Reset</Button>
          </div>
        </div>
      </div>
      <p class="muted hint">Saving, restoring and resetting fail while a stream is running.</p>
    </div>
  {:else}
    <LoadState error={loadError} what="the display layout" onretry={load} />
  {/if}
</Panel>

<style>
  .body {
    container-type: inline-size;
  }
  .columns {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-5);
  }
  .group {
    display: grid;
    gap: var(--space-3);
    align-content: start;
  }
  .recovery {
    padding-top: var(--space-4);
    border-top: 1px solid var(--line);
  }
  @container (min-width: 760px) {
    .columns {
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: var(--space-6);
    }
    .recovery {
      padding-top: 0;
      padding-left: var(--space-6);
      border-top: 0;
      border-left: 1px solid var(--line);
    }
  }
  h4 {
    margin: 0;
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
  }
  .text {
    font-size: var(--text-sm);
  }
  .hint {
    font-size: var(--text-xs);
    line-height: 1.45;
  }
  .action {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--space-3);
    flex-wrap: wrap;
  }
  .action > div {
    flex: 1 1 220px;
    display: grid;
    gap: 2px;
  }
</style>
