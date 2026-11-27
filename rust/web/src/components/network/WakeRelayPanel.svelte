<script lang="ts">
  // Settings › Network: the devices enrolled as away wake relays, each with Remove, Rotate keys
  // for all of them, and how to add one. Status from GET /api/wake-relay.
  import { onMount, untrack } from 'svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import CopyButton from '../CopyButton.svelte';
  import EmptyState from '../EmptyState.svelte';
  import LoadState from '../LoadState.svelte';
  import Panel from '../Panel.svelte';
  import StatusPill from '../StatusPill.svelte';
  import { api, type WakeRelayPeer, type WakeRelayStatus } from '../../lib/api';
  import { confirm, notify } from '../../lib/feedback.svelte';
  import { poll } from '../../lib/format';
  import { link } from '../../lib/router.svelte';
  import { reason } from '../maintenance/util';
  import { RELAY_COMMAND, relayName, relaySeen, relayStale } from './wake-relay';

  let {
    initial = null,
    saved = null,
  }: {
    initial?: WakeRelayStatus | null;
    /** The settings as last saved: a new value loads the status again, as the switch may have changed. */
    saved?: unknown;
  } = $props();

  const ROTATE = 'rotate';
  let status = $state<WakeRelayStatus | null>(untrack(() => initial));
  let loadError = $state('');
  /** What failed last (Remove or Rotate keys) and why, in the host's words. */
  let actionError = $state<{ title: string; message: string } | null>(null);
  /** The relay being removed, or `ROTATE`. */
  let busy = $state<string | null>(null);
  let now = $state(Date.now() / 1000);
  /** Tolerates an answer without the list, such as a test fixture's. */
  const relays = $derived(status?.relays ?? []);

  async function load() {
    try {
      status = await api.wakeRelay.status();
      loadError = '';
    } catch (error) {
      loadError = reason(error);
    }
    now = Date.now() / 1000;
  }

  // Every minute while the page shows, so "last checked in" and a relay that stopped stay current.
  onMount(() => poll(load, 60_000));

  let seenSaved = untrack(() => saved);
  $effect(() => {
    if (saved === seenSaved) return;
    seenSaved = saved;
    untrack(load);
  });

  async function change(id: string, action: () => Promise<WakeRelayStatus>, done: string, failure: string) {
    busy = id;
    actionError = null;
    try {
      status = await action();
      now = Date.now() / 1000;
      notify(done, 'ok');
    } catch (error) {
      actionError = { title: failure, message: reason(error) };
    } finally {
      busy = null;
    }
  }

  async function remove(relay: WakeRelayPeer) {
    const name = relayName(relay);
    const ok = await confirm({
      title: `Remove ${name}?`,
      message: `${name} stops waking this PC at its next check-in, within five minutes. To keep it out for good, also turn off its Host control; to add it back, run ${RELAY_COMMAND} --enroll on it at home.`,
      confirm: 'Remove',
      danger: true,
    });
    if (ok) {
      await change(relay.id, () => api.wakeRelay.revoke(relay.id), `${name} no longer wakes this PC.`, `Removing ${name} failed`);
    }
  }

  async function rotate() {
    const ok = await confirm({
      title: 'Rotate the wake relay keys?',
      message:
        'Each device must connect to this PC once to get the new keys before it can wake it from away again. Relays pick them up at their next check-in, within five minutes while this PC is awake.',
      confirm: 'Rotate keys',
      danger: true,
    });
    if (ok) {
      await change(ROTATE, () => api.wakeRelay.rotate(), 'New keys made. Connect each device to this PC once.', 'Rotating the keys failed');
    }
  }

  const added = (relay: WakeRelayPeer) => new Date(relay.enrolled_unix * 1000).toLocaleDateString();
</script>

{#snippet steps()}
  <ol class="steps">
    <li>On an always-on computer on this network, install Nightjar's desktop client and pair it with this PC.</li>
    <li>Turn on Host control for it on the <a href="/devices" use:link>Devices</a> page.</li>
    <li>
      <span>Run <code>{RELAY_COMMAND}</code> on it and leave it running.</span>
      <CopyButton value={RELAY_COMMAND} label="Copy the command" what="the command" />
    </li>
  </ol>
{/snippet}

<Panel
  id="away-wake-relay"
  title="Away wake relay"
  level={3}
  description="Always-on devices on this network that wake this PC for your devices when they are away."
  flush
>
  {#snippet actions()}
    {#if relays.length}
      <Button size="sm" icon="key" busy={busy === ROTATE} disabled={busy !== null} onclick={rotate}>Rotate keys</Button>
    {/if}
  {/snippet}
  {#if status}
    {#if actionError}
      <div class="pad">
        <Banner tone="danger" live title={actionError.title} ondismiss={() => (actionError = null)}>{actionError.message}</Banner>
      </div>
    {/if}
    {#if loadError}
      <div class="pad"><LoadState error={loadError} what="the wake relays" stale onretry={load} /></div>
    {/if}
    {#if !status.enabled}
      <p class="pad off muted">Off. Turn on Play over the internet, then Away wake relay, under Discovery and internet above.</p>
    {/if}
    {#if relays.length}
      <ul>
        {#each relays as relay (relay.id)}
          {@const stale = relayStale(relay, now)}
          <li>
            <div class="who">
              <div class="name">
                <span>{relayName(relay)}</span>
                {#if stale}
                  <StatusPill status="warning" label="Not refreshing" />
                {:else}
                  <StatusPill status="ready" />
                {/if}
              </div>
              <div class="meta muted">
                <span>{relaySeen(relay)}</span>
                <span aria-hidden="true">·</span>
                <span>Added {added(relay)}</span>
              </div>
              {#if stale && status.enabled}
                <p class="hint muted">
                  Check that it is on, on this network and still running <code>{RELAY_COMMAND}</code>. If it stopped after a refusal, run
                  <code>{RELAY_COMMAND} --enroll</code> on it.
                </p>
              {/if}
            </div>
            <Button size="sm" variant="danger" busy={busy === relay.id} disabled={busy !== null} onclick={() => remove(relay)}>
              Remove
            </Button>
          </li>
        {/each}
      </ul>
      {#if status.enabled}
        <details class="pad more">
          <summary>Add another relay</summary>
          {@render steps()}
        </details>
      {/if}
    {:else if status.enabled}
      <EmptyState icon="power" title="No wake relay yet">
        <p>One always-on computer at home is enough to wake this PC from anywhere.</p>
      </EmptyState>
      <div class="pad setup">{@render steps()}</div>
    {/if}
  {:else}
    <div class="pad"><LoadState error={loadError} what="the wake relays" onretry={load} /></div>
  {/if}
</Panel>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  ul > li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-5);
    border-bottom: 1px solid var(--line);
  }
  ul > li:last-child {
    border-bottom: 0;
  }
  .who {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .name {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    overflow-wrap: anywhere;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0 var(--space-1-5);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
  .hint {
    margin: 0;
    font-size: var(--text-xs);
  }
  .pad {
    padding: var(--space-4) var(--space-5);
  }
  .off {
    margin: 0;
    font-size: var(--text-sm);
  }
  .more {
    padding-block: var(--space-1);
    border-top: 1px solid var(--line);
    font-size: var(--text-sm);
  }
  .more summary {
    font-weight: var(--weight-medium);
  }
  .more .steps {
    margin-block: var(--space-1) var(--space-3);
  }
  .setup {
    padding-top: 0;
  }
  .steps {
    display: grid;
    gap: var(--space-2);
    max-width: 70ch;
    margin: 0;
    padding-inline-start: var(--space-5);
    font-size: var(--text-sm);
    color: var(--ink-2);
  }
  .steps li > span {
    margin-inline-end: var(--space-2);
  }
  /* Lists reach the panel's edges, so they follow its narrower padding on a phone. */
  @media (max-width: 520px) {
    ul > li,
    .pad {
      padding-inline: var(--space-4);
    }
  }
</style>
