<script lang="ts">
  // Invites to play over the internet (docs/multiclient.md): a link that pairs one friend's
  // Nightjar as a guest, the links waiting and the friends paired from one.
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import Select from '../Select.svelte';
  import { api, type GuestInvites, type GuestPermission, type InviteCreated } from '../../lib/api';
  import { copyText } from '../../lib/clipboard';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { INVITE_LIFETIMES, PERMISSIONS, endsIn, inviteMessage, inviteSeconds, permissionLabel } from '../../lib/session-guests';

  let {
    invites,
    onchange,
  }: {
    invites: GuestInvites;
    onchange?: (invites: GuestInvites) => void;
  } = $props();

  // A reply without its lists (an older host, a proxy's own answer) is an empty list, not a crash.
  const links = $derived(invites.links ?? []);
  const friends = $derived(invites.friends ?? []);

  let open = $state(false);
  let permission = $state<GuestPermission>('controller');
  let lifetime = $state('24h');
  let made = $state<InviteCreated | null>(null);
  let running = $state<string | null>(null);

  async function create() {
    running = 'create';
    try {
      made = await api.sessionGuests.invite(permission, inviteSeconds(lifetime));
    } catch (error) {
      failed('Couldn’t make an invite', error);
      return;
    } finally {
      running = null;
    }
    // The link is made and shown. A list that cannot be read now is the page's next poll's to bring: saying the
    // invite failed here would have the owner make a second one while the first still works.
    try {
      onchange?.(await api.sessionGuests.invites());
    } catch {
      // The poll reads it again.
    }
  }

  async function copy() {
    if (!made) return;
    try {
      await copyText(inviteMessage(made.link));
      notify('Invite copied: send it to one person', 'ok');
    } catch (error) {
      failed('Couldn’t copy the invite', error);
    }
  }

  async function takeBack(id: number) {
    running = `link:${id}`;
    try {
      onchange?.(await api.sessionGuests.revokeInvite(id));
      if (made?.id === id) made = null;
      notify('The invite no longer works', 'ok');
    } catch (error) {
      failed('Couldn’t take the invite back', error);
    } finally {
      running = null;
    }
  }

  async function remove(uuid: string, name: string) {
    const sure = await confirm({
      title: `Remove ${name}?`,
      message: `${name}’s stream ends and the device can no longer reach this PC. Send a new invite to play again.`,
      confirm: 'Remove',
      danger: true,
    });
    if (!sure) return;
    running = `friend:${uuid}`;
    try {
      onchange?.(await api.sessionGuests.removeInvited(uuid));
      notify(`${name} was removed`, 'ok');
    } catch (error) {
      failed(`Couldn’t remove ${name}`, error);
    } finally {
      running = null;
    }
  }
</script>

<Panel
  title="Invite a friend over the internet"
  description="A link that lets one person with Nightjar join the games you play, from anywhere. No port forwarding and no PIN."
>
  <div class="card">
    {#if !open}
      <div>
        <Button size="sm" onclick={() => (open = true)}>Invite a friend</Button>
      </div>
    {:else}
      <div class="form">
        <Select
          aria-label="What your friend may do"
          value={permission}
          options={[...PERMISSIONS]}
          onchange={(value) => (permission = value as GuestPermission)}
        />
        <Select
          aria-label="How long the invite works"
          value={lifetime}
          options={INVITE_LIFETIMES.map(({ value, label }) => ({ value, label }))}
          onchange={(value) => (lifetime = value)}
        />
        <Button size="sm" busy={running === 'create'} disabled={running !== null} onclick={create}>Create link</Button>
      </div>
      <p class="muted">
        Your friend opens the link in Nightjar, or pastes it under Add PC. Their device pairs as a guest: it can only join a
        game someone is playing here, and only when that player lets it in. When the time is up the device is removed.
        While an invite works, this PC looks for your friend over the internet even when Play over the internet is off.
      </p>
      {#if made}
        <div class="made">
          <input class="link" readonly value={made.link} aria-label="Invite link" onfocus={(event) => event.currentTarget.select()} />
          <Button size="sm" onclick={copy}>Copy invite</Button>
        </div>
        <p class="muted">Send it to one person only: it works once. It is shown only now.</p>
      {/if}
    {/if}
    {#if links.length}
      <section aria-labelledby="invites-waiting">
        <h3 id="invites-waiting">Links not used yet</h3>
        <ul class="rows">
          {#each links as link (link.id)}
            <li>
              <span class="who">
                <strong>{permissionLabel(link.permission)}</strong>
                <span class="muted num">{endsIn(link.expires)}</span>
              </span>
              <span class="controls">
                <Button
                  size="sm"
                  variant="ghost"
                  busy={running === `link:${link.id}`}
                  disabled={running !== null}
                  onclick={() => takeBack(link.id)}>Take back</Button
                >
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
    {#if friends.length}
      <section aria-labelledby="invites-friends">
        <h3 id="invites-friends">Invited devices</h3>
        <ul class="rows">
          {#each friends as friend (friend.uuid)}
            <li>
              <span class="who">
                <strong>{friend.name}</strong>
                <span class="muted">{permissionLabel(friend.permission)}</span>
                <span class="muted num">{endsIn(friend.until)}</span>
              </span>
              <span class="controls">
                <Button
                  size="sm"
                  variant="danger"
                  busy={running === `friend:${friend.uuid}`}
                  disabled={running !== null}
                  onclick={() => remove(friend.uuid, friend.name)}>Remove</Button
                >
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  </div>
</Panel>

<style>
  .card {
    display: grid;
    gap: var(--space-3);
  }
  .form,
  .made {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }
  .link {
    flex: 1 1 16rem;
    min-width: 0;
    font-family: var(--font-mono, monospace);
    font-size: var(--text-sm);
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
    margin: 0;
    color: var(--ink-2);
    font-size: var(--text-sm);
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  @media (max-width: 40rem) {
    .rows li {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
