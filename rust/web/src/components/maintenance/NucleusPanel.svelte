<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte';
  import Badge from '../Badge.svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import LoadState from '../LoadState.svelte';
  import Panel from '../Panel.svelte';
  import { api, type CoopPlayer, type CoopSession, type CoopStatus } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { statusLoader } from './status-loader';
  import { endText, installStates, kickable, phaseOf, playerText, polls } from './coop';
  import { reason } from './util';

  /** A status to show before the first load (the tests render with one). */
  let { initial = null }: { initial?: CoopStatus | null } = $props();

  let status = $state<CoopStatus | null>(untrack(() => initial));
  let loadError = $state('');
  /** The action in flight, as `kick:<session>:<slot>` or `end:<session>`. */
  let running = $state<string | null>(null);
  let timer: ReturnType<typeof setInterval> | undefined;

  const install = $derived(status ? installStates[status.install.state] : null);

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

  async function run(action: string, task: () => Promise<unknown>, done: string, failure: string) {
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

  async function kick(session: CoopSession, player: CoopPlayer) {
    const ok = await confirm({
      title: `Remove ${player.name ?? 'this device'}?`,
      message: `Its stream of ${session.title} ends and its slot is free for another device. The device cannot join this session again.`,
      confirm: 'Remove',
      danger: true,
    });
    if (ok)
      await run(
        `kick:${session.id}:${player.slot}`,
        () => api.coop.kick(session.id, player.slot),
        `${player.name ?? 'The device'} was removed.`,
        'Removing the player failed',
      );
  }

  async function end(session: CoopSession) {
    const ok = await confirm({
      title: `End ${session.title}?`,
      message: 'Nucleus Co-op and every game instance close, and each player’s stream ends. Unsaved progress is lost.',
      confirm: 'End session',
      danger: true,
    });
    if (ok) await run(`end:${session.id}`, () => api.coop.end(session.id), 'The session is ending.', 'Ending the session failed');
  }

  onMount(() => {
    if (!status) load();
    timer = setInterval(() => {
      if (status && polls(status) && !running) loader.tick();
    }, 3000);
  });
  onDestroy(() => clearInterval(timer));
</script>

<Panel
  title="Nucleus Co-op"
  level={3}
  description="Runs background streams and co-op games, one game instance per device, while you keep using this PC."
>
  {#if status}
    <div class="stack">
      <dl class="facts">
        <dt>Background streaming</dt>
        <dd>{status.mode.label}</dd>
        <dt>Why</dt>
        <dd class="muted">{status.mode.reason}</dd>
        <dt>Nucleus Co-op</dt>
        <dd>
          {#if install}<Badge tone={install.tone}>{install.label}</Badge>{/if}
          {#if status.install.version}<span class="muted">{status.install.version}</span>{/if}
          {#if status.install.path}<span class="muted mono">{status.install.path}</span>{/if}
        </dd>
      </dl>
      {#if !status.available && status.reason}
        <Banner tone="warn" title="Nucleus Co-op sessions are unavailable">{status.reason}</Banner>
      {/if}
      {#if status.install.state === 'failed' && status.install.error}
        <Banner tone="danger" title="Installing Nucleus Co-op failed">{status.install.error} The next launch tries again.</Banner>
      {:else if status.install.state === 'missing'}
        <p class="muted">
          The first background or co-op launch downloads Nucleus Co-op and checks it before it runs. Add game handlers in
          Nucleus Co-op’s handler hub; each game with one gets a “Co-op (Nucleus)” entry in the library.
        </p>
      {/if}
      {#if status.sessions.length === 0}
        <p class="muted">No session runs.</p>
      {:else}
        <ul class="sessions">
          {#each status.sessions as session (session.id)}
            {@const phase = phaseOf(session.phase)}
            <li class="stack">
              <div class="row">
                <strong>{session.title}</strong>
                <Badge tone="neutral">{session.kind === 'coop' ? 'Co-op' : 'Background stream'}</Badge>
                <Badge tone={phase.tone}>{phase.label}</Badge>
              </div>
              {#if session.end}
                <p class="muted">{endText(session.end)}</p>
              {/if}
              <ul class="players">
                {#each session.players as player (player.slot)}
                  <li class="row">
                    <span class={player.device ? '' : 'muted'}>{playerText(player)}</span>
                    {#if player.away}<Badge tone="warn">Away, slot kept</Badge>{/if}
                    {#if session.kind === 'coop' && kickable(session, player)}
                      <Button
                        size="sm"
                        variant="ghost"
                        busy={running === `kick:${session.id}:${player.slot}`}
                        onclick={() => kick(session, player)}>Remove</Button
                      >
                    {/if}
                  </li>
                {/each}
              </ul>
              {#if session.phase !== 'ended'}
                <div class="row">
                  <Button variant="danger" busy={running === `end:${session.id}`} onclick={() => end(session)}>End session</Button>
                </div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
      <ul class="muted hint">
        <li>Only controllers reach the game; keyboard, mouse and touch stay off. At most 4 players.</li>
        <li>A device joins a co-op game started by another one only with Join co-op on its device page.</li>
      </ul>
    </div>
  {:else}
    <LoadState error={loadError} what="the Nucleus Co-op status" onretry={load} />
  {/if}
</Panel>

<style>
  .hint {
    font-size: var(--text-xs);
  }
  .sessions,
  .players {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--space-2);
  }
  .sessions > li + li {
    border-top: 1px solid var(--line);
    padding-top: var(--space-3);
  }
</style>
