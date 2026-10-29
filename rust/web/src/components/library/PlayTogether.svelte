<script lang="ts">
  // The game page's co-op: set Nucleus Co-op up, get the game's handler, then play together.
  // States come from lib/coop-view.ts; this file only draws them and runs the two actions.
  import { onMount, untrack } from 'svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import LoadState from '../LoadState.svelte';
  import Panel from '../Panel.svelte';
  import Toggle from '../Toggle.svelte';
  import { api, type CoopStatus } from '../../lib/api';
  import { AWAY_FROM_PC, COOP_LIMITS, OPEN_ON_PC, playTogether, pollsView } from '../../lib/coop-view';
  import { failed, notify } from '../../lib/feedback.svelte';
  import { poll } from '../../lib/format';
  import { statusLoader } from '../maintenance/status-loader';
  import { reason } from '../maintenance/util';
  import type { AppDraft } from './draft.svelte';

  let {
    uuid,
    draft,
    initial = null,
  }: {
    /** The saved app's uuid. */
    uuid: string;
    /** The app form: the library entry switch saves with it. */
    draft: AppDraft;
    /** A status to show before the first load (the tests render with one). */
    initial?: CoopStatus | null;
  } = $props();

  let status = $state<CoopStatus | null>(untrack(() => initial));
  let loadError = $state('');
  /** 'install', 'open' or 'close' while its request is out. */
  let busy = $state('');

  const view = $derived(status ? playTogether(status, uuid) : null);

  const loader = statusLoader(
    () => api.coop.status(),
    (value) => {
      status = value;
      loadError = '';
    },
    (error) => {
      loadError = reason(error);
    },
  );
  const load = loader.load;

  async function setUp() {
    busy = 'install';
    try {
      await api.coop.install();
    } catch (error) {
      failed('Setting up co-op failed', error);
    } finally {
      busy = '';
    }
    await load();
  }

  async function open() {
    busy = 'open';
    try {
      await api.coop.open();
      notify('Nucleus Co-op is opening on the PC’s screen.', 'ok');
    } catch (error) {
      failed('Opening Nucleus Co-op failed', error);
    } finally {
      busy = '';
    }
    await load();
  }

  async function close() {
    busy = 'close';
    try {
      const { closed } = await api.coop.close();
      notify(closed ? 'Nucleus Co-op closed on the PC.' : 'Nucleus Co-op was already closed.', 'ok');
    } catch (error) {
      failed('Closing Nucleus Co-op failed', error);
    } finally {
      busy = '';
    }
    await load();
  }

  onMount(() => {
    if (!status) load();
    // Marked as a poll and paused while the tab is hidden, so a page left open lets the PC sleep.
    return poll(() => {
      if (view && pollsView(view) && !busy) return loader.tick();
    }, 3000);
  });
</script>

{#snippet closeAction()}
  <Button busy={busy === 'close'} disabled={busy !== '' && busy !== 'close'} onclick={close}>Close Nucleus Co-op</Button>
{/snippet}

{#if loadError && !status}
  <Panel title="Play together">
    <LoadState error={loadError} what="the co-op status" onretry={load} />
  </Panel>
{:else if view && view.kind !== 'hidden'}
  <Panel title="Play together">
    <div class="play">
      {#if view.kind === 'unavailable'}
        <Banner tone="warn" title="Co-op can’t run on this PC">{view.reason}</Banner>
      {:else if view.kind === 'launcher'}
        <p class="muted">{view.text}</p>
      {:else if view.kind === 'setup'}
        <p class="lead">{view.text}</p>
        <div class="action">
          <Button variant="primary" icon="download" busy={busy === 'install'} onclick={setUp}>Set up co-op</Button>
          <span class="muted note">{view.download}</span>
        </div>
      {:else if view.kind === 'installing'}
        <p class="lead" role="status">{view.label}</p>
        <div
          class="meter"
          role="progressbar"
          aria-label="Nucleus Co-op download"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={view.percent ?? undefined}
          class:indeterminate={view.percent === null}
        >
          <span class="fill" style:transform="scaleX({(view.percent ?? 30) / 100})"></span>
        </div>
      {:else if view.kind === 'failed'}
        <Banner tone="danger" title="Setting up co-op failed" live>
          {view.error}
          {#snippet actions()}
            <Button busy={busy === 'install'} onclick={setUp}>Try again</Button>
          {/snippet}
        </Banner>
      {:else if view.kind === 'broken'}
        <Banner tone="danger" title="Nucleus Co-op’s handlers didn’t load">{view.error}</Banner>
      {:else if view.kind === 'taken'}
        <Banner tone="warn" title="Co-op isn’t in the library">{view.reason}</Banner>
      {:else if view.kind === 'anti_cheat'}
        <Banner tone="warn" title="No co-op or background streams for this game">{view.reason}</Banner>
      {:else if view.kind === 'checking'}
        <p class="muted" role="status">Checking Nucleus Co-op’s handlers…</p>
      {:else if view.kind === 'handler'}
        <p class="lead">{view.text}</p>
        {#if view.runs}<p class="muted note">{view.runs}</p>{/if}
        <div class="action">
          <Button variant="primary" busy={busy === 'open'} disabled={status?.opened === true} onclick={open}>Open Nucleus Co-op on this PC</Button>
          {#if status?.opened}{@render closeAction()}{/if}
        </div>
        <p class="muted note">{view.steps}</p>
        <details class="away">
          <summary>Using this console away from the PC?</summary>
          <p class="muted note">{AWAY_FROM_PC}</p>
        </details>
      {:else if view.kind === 'ready'}
        {#if view.refusal}
          <Banner tone="warn" title="No co-op or background streams on this account">{view.refusal}</Banner>
        {:else}
          <p class="lead"><strong>{view.players}</strong> {view.start}</p>
        {/if}
        <Toggle
          label="Show “{view.entry}” in the library"
          hint="Saves with the app."
          bind:checked={() => draft.flag('coop-entry'), (value) => draft.setFlag('coop-entry', value)}
        />
        <p class="muted note">{COOP_LIMITS}</p>
        {#if view.runs}<p class="muted note">{view.runs}</p>{/if}
        {#if status?.opened}
          <div class="action">
            <span class="muted note">{OPEN_ON_PC}</span>
            {@render closeAction()}
          </div>
        {/if}
      {/if}
    </div>
  </Panel>
{/if}

<style>
  .play {
    display: grid;
    gap: var(--space-4);
    max-width: 68ch;
  }
  .lead {
    margin: 0;
    line-height: var(--leading);
  }
  .note {
    margin: 0;
    font-size: var(--text-xs);
    line-height: var(--leading-snug);
  }
  .action {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2) var(--space-4);
  }
  .meter {
    position: relative;
    height: 8px;
    border-radius: var(--radius-pill);
    background: var(--sunken);
    overflow: hidden;
  }
  .fill {
    position: absolute;
    inset: 0;
    background: var(--accent);
    transform-origin: left center;
    transition: transform var(--duration-base) var(--ease-standard);
  }
  .indeterminate .fill {
    animation: sweep 1.4s var(--ease-standard) infinite;
  }
  @keyframes sweep {
    from {
      transform: translateX(-100%) scaleX(0.3);
    }
    to {
      transform: translateX(100%) scaleX(0.3);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .indeterminate .fill {
      animation: none;
    }
  }
  .away summary {
    cursor: pointer;
    font-weight: var(--weight-medium);
    font-size: var(--text-sm);
    padding-block: var(--space-2);
  }
  .away p {
    margin-top: var(--space-1);
  }
</style>
