<script lang="ts">
  // Guests from paired devices (docs/multiclient.md): devices that joined the running game and
  // devices waiting to. The console answers for the game as the device playing can.
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import Select from '../Select.svelte';
  import { api, type GuestPermission, type SessionGuestsStatus } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { PERMISSIONS, STAYS, endsIn, guestLine, staySeconds, usesKeyboardMouse } from '../../lib/session-guests';

  let {
    status,
    onchange,
  }: {
    status: SessionGuestsStatus;
    onchange?: (status: SessionGuestsStatus) => void;
  } = $props();

  /** The action in flight, as `<what>:<id>`. */
  let running = $state<string | null>(null);
  /** The permission and stay chosen for each waiting device, by request id. */
  let chosen = $state<Record<number, GuestPermission>>({});
  let stays = $state<Record<number, string>>({});

  const owner = $derived(status.owner_name ?? 'the device playing');
  const description = $derived(
    `Playing on ${owner} · ${status.guests.length} of ${status.max_guests} guests`,
  );

  async function run(action: string, task: () => Promise<SessionGuestsStatus>, done: string, failure: string) {
    running = action;
    try {
      const next = await task();
      notify(done, 'ok');
      onchange?.(next);
    } catch (error) {
      failed(failure, error);
    } finally {
      running = null;
    }
  }

  const approve = (id: number, name: string) =>
    run(
      `approve:${id}`,
      () => api.sessionGuests.approve(id, chosen[id] ?? status.default_permission, staySeconds(stays[id] ?? 'game')),
      `${name} joined`,
      `Couldn’t let ${name} in`,
    );
  const deny = (id: number, name: string) =>
    run(`deny:${id}`, () => api.sessionGuests.deny(id), `${name} was not let in`, `Couldn’t turn ${name} down`);
  const kick = (id: number, name: string) =>
    run(`kick:${id}`, () => api.sessionGuests.kick(id), `${name} left the game`, `Couldn’t remove ${name}`);
  async function revoke(id: number, name: string) {
    const sure = await confirm({
      title: `Block ${name} from this game?`,
      message: `${name}’s stream ends and it can’t join again until the game ends.`,
      confirm: 'Block',
      danger: true,
    });
    if (sure) await run(`revoke:${id}`, () => api.sessionGuests.revoke(id), `${name} can’t join this game`, `Couldn’t block ${name}`);
  }
  const permission = (id: number, name: string, value: string) =>
    run(
      `permission:${id}`,
      () => api.sessionGuests.permission(id, value as GuestPermission),
      `${name} can now: ${PERMISSIONS.find((p) => p.value === value)?.label ?? value}`,
      `Couldn’t change what ${name} may do`,
    );
  const keyboardMouse = (id: number | null) =>
    run(
      `kbm:${id ?? 'owner'}`,
      () => api.sessionGuests.keyboardMouse(id),
      id == null ? `The keyboard and mouse went back to ${owner}` : 'Keyboard and mouse handed over',
      'Couldn’t hand over the keyboard and mouse',
    );
</script>

<Panel title="Guests" {description}>
  <div class="card">
    {#if status.waiting.length}
      <section aria-labelledby="guests-waiting">
        <h3 id="guests-waiting">Waiting to join</h3>
        <ul class="rows">
          {#each status.waiting as request (request.id)}
            <li>
              <span class="who">
                <strong>{request.name}</strong>
                <span class="muted num">{request.seconds_left} s left</span>
              </span>
              <span class="controls">
                <Select
                  aria-label="What {request.name} may do"
                  value={chosen[request.id] ?? status.default_permission}
                  options={[...PERMISSIONS]}
                  onchange={(value) => (chosen[request.id] = value as GuestPermission)}
                />
                <Select
                  aria-label="How long {request.name} may stay"
                  value={stays[request.id] ?? 'game'}
                  options={STAYS.map(({ value, label }) => ({ value, label }))}
                  onchange={(value) => (stays[request.id] = value)}
                />
                <Button
                  size="sm"
                  busy={running === `approve:${request.id}`}
                  disabled={running !== null}
                  onclick={() => approve(request.id, request.name)}>Let in</Button
                >
                <Button
                  size="sm"
                  variant="ghost"
                  busy={running === `deny:${request.id}`}
                  disabled={running !== null}
                  onclick={() => deny(request.id, request.name)}>Deny</Button
                >
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
    {#if status.guests.length}
      <ul class="rows" aria-label="Guests">
        {#each status.guests as guest (guest.id)}
          <li>
            <span class="who">
              <strong>{guest.name}</strong>
              {#if guest.streaming}<Badge tone="live">Playing</Badge>{:else}<Badge tone="warn">Away</Badge>{/if}
              <span class="muted">{guestLine(status, guest.id)}</span>
              {#if guest.expires}<span class="muted num">{endsIn(guest.expires)}</span>{/if}
            </span>
            <span class="controls">
              <Select
                aria-label="What {guest.name} may do"
                value={guest.permission}
                options={[...PERMISSIONS]}
                disabled={running !== null}
                onchange={(value) => permission(guest.id, guest.name, value)}
              />
              {#if usesKeyboardMouse(guest.permission) && guest.streaming}
                {#if status.keyboard_mouse === guest.id}
                  <Button size="sm" variant="ghost" disabled={running !== null} onclick={() => keyboardMouse(null)}
                    >Take back keyboard</Button
                  >
                {:else}
                  <Button size="sm" variant="ghost" disabled={running !== null} onclick={() => keyboardMouse(guest.id)}
                    >Give keyboard</Button
                  >
                {/if}
              {/if}
              <Button
                size="sm"
                variant="ghost"
                busy={running === `kick:${guest.id}`}
                disabled={running !== null}
                onclick={() => kick(guest.id, guest.name)}>Kick</Button
              >
              <Button
                size="sm"
                variant="danger"
                busy={running === `revoke:${guest.id}`}
                disabled={running !== null}
                onclick={() => revoke(guest.id, guest.name)}>Block</Button
              >
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</Panel>

<style>
  .card {
    display: grid;
    gap: var(--space-4);
  }
  h3 {
    margin: 0 0 var(--space-2);
    font-size: var(--text-md);
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .rows li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--space-2) var(--space-3);
    min-height: var(--control-height);
    padding-block: var(--space-2);
  }
  .rows li + li {
    border-top: 1px solid var(--line);
  }
  .who,
  .controls {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
    min-width: 0;
  }
  .who strong {
    overflow-wrap: anywhere;
  }
  .muted {
    color: var(--ink-2);
    font-size: var(--text-sm);
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  /* On a phone the controls go under the name, all of them reachable. */
  @media (max-width: 40rem) {
    .rows li {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
