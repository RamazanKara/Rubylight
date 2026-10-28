<script lang="ts">
  import { onDestroy } from 'svelte';
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import Panel from '../Panel.svelte';
  import { api, type Client, type CoopSession } from '../../lib/api';
  import {
    endConfirmation,
    headcount,
    invitees,
    joinSteps,
    removeConfirmation,
    seats,
    type Invitee,
    type Seat,
  } from '../../lib/coop-view';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { link } from '../../lib/router.svelte';

  let {
    session,
    clients,
    clientsError = '',
    at = Date.now(),
    onchange,
  }: {
    session: CoopSession;
    /** The paired devices, for the invite list; null while they load. */
    clients: Client[] | null;
    clientsError?: string;
    /** When `session` was read: an away device's time counts down from there. */
    at?: number;
    onchange?: () => void | Promise<void>;
  } = $props();

  /** The action in flight: `remove:<slot>`, `join:<uuid>` or `end`. */
  let running = $state<string | null>(null);
  let now = $state(Date.now());
  // The away countdown ticks between polls, only while someone is away.
  const anyAway = $derived(session.players.some((player) => player.device && player.away));
  let timer: ReturnType<typeof setInterval> | undefined;
  $effect(() => {
    if (!anyAway) return;
    timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });
  onDestroy(() => clearInterval(timer));

  const list = $derived(seats(session, Math.max(0, (now - at) / 1000)));
  const invite = $derived(clients ? invitees(session, clients) : []);
  const hasFree = $derived(session.players.some((player) => !player.device));

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
    await onchange?.();
  }

  async function remove(seat: Seat) {
    if (!(await confirm(removeConfirmation(seat)))) return;
    await run(
      `remove:${seat.slot}`,
      () => api.coop.kick(session.id, seat.slot),
      `${seat.name ?? seat.label} left ${session.title}.`,
      'Removing the player failed',
    );
  }

  async function letJoin(device: Invitee) {
    await run(
      `join:${device.uuid}`,
      () => api.clients.update({ uuid: device.uuid, coop_join: true }),
      `${device.name} can join. ${joinSteps(session)}`,
      `Letting ${device.name} join failed`,
    );
  }

  async function end() {
    if (!(await confirm(endConfirmation(session)))) return;
    await run('end', () => api.coop.end(session.id), `Co-op in ${session.title} is ending.`, 'Ending co-op failed');
  }
</script>

{#snippet endAction()}
  <Button variant="danger" size="sm" icon="stop" busy={running === 'end'} disabled={running !== null && running !== 'end'} onclick={end}>
    End co-op
  </Button>
{/snippet}

<Panel title={session.title} description="Co-op · {headcount(session)}" actions={endAction}>
  <div class="card">
    <ul class="seats" aria-label="Players">
      {#each list as seat (seat.slot)}
        <li class:free={!seat.device}>
          <span class="number">{seat.label}</span>
          {#if seat.device}
            <span class="who">
              <strong>{seat.name}</strong>
              {#if seat.keeps}
                <Badge tone="warn">Away</Badge><span class="keeps">{seat.keeps}</span>
              {:else}
                <Badge tone="live">Playing</Badge>
              {/if}
            </span>
            <Button
              size="sm"
              variant="ghost"
              busy={running === `remove:${seat.slot}`}
              disabled={running !== null && running !== `remove:${seat.slot}`}
              onclick={() => remove(seat)}>Remove</Button
            >
          {:else}
            <span class="who muted">Free slot</span>
          {/if}
        </li>
      {/each}
    </ul>

    {#if hasFree}
      <section class="invite" aria-labelledby="invite-{session.id}">
        <h3 id="invite-{session.id}">Invite a player</h3>
        {#if clientsError}
          <p class="muted">Can’t list the paired devices: {clientsError}</p>
        {:else if !clients}
          <p class="muted">Loading paired devices…</p>
        {:else if invite.length}
          <ul class="devices">
            {#each invite as device (device.uuid)}
              <li>
                <span class="name">{device.name}</span>
                {#if device.canJoin}
                  <span class="muted can"><Icon name="check" size={15} />Can join</span>
                {:else}
                  <Button
                    size="sm"
                    busy={running === `join:${device.uuid}`}
                    disabled={running !== null && running !== `join:${device.uuid}`}
                    onclick={() => letJoin(device)}>Let {device.name} join</Button
                  >
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
        <p class="steps">{joinSteps(session)}</p>
        <a class="pair" href="/devices" use:link><Icon name="plus" size={15} />Pair a new device</a>
      </section>
    {/if}
  </div>
</Panel>

<style>
  .card {
    display: grid;
    gap: var(--space-5);
  }
  .seats,
  .devices {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
  }
  /* Columns, not wrapping: on a phone the action stays on its row's right, beside the name it acts on. */
  .seats li,
  .devices li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--space-2) var(--space-3);
    min-height: var(--control-height);
    padding-block: var(--space-2);
  }
  .seats li {
    grid-template-columns: 5.5rem minmax(0, 1fr) auto;
  }
  .seats li + li,
  .devices li + li {
    border-top: 1px solid var(--line);
  }
  .number {
    color: var(--ink-2);
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
  }
  .who {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
    min-width: 0;
  }
  .who strong,
  .name {
    overflow-wrap: anywhere;
  }
  .free .who {
    font-size: var(--text-sm);
  }
  .keeps {
    font-size: var(--text-sm);
    color: var(--ink-2);
    font-variant-numeric: tabular-nums;
  }
  .invite {
    display: grid;
    gap: var(--space-2);
    border-top: 1px solid var(--line);
    padding-top: var(--space-4);
  }
  h3 {
    margin: 0;
    font-size: var(--text-md);
  }
  .name {
    min-width: 0;
  }
  @media (max-width: 30rem) {
    .seats li {
      grid-template-columns: 4.5rem minmax(0, 1fr) auto;
    }
  }
  .can {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--text-sm);
  }
  .steps {
    margin: 0;
    font-size: var(--text-sm);
    color: var(--ink-2);
  }
  .pair {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    justify-self: start;
    font-size: var(--text-sm);
    min-height: var(--control-height);
  }
</style>
