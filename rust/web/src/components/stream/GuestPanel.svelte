<script lang="ts">
  // The Stream page's guests (docs/browser-guests.md): invite links, who watches, kick and
  // revoke. A link's token is shown once, right after it is made: the host keeps only its hash.
  import { onMount } from 'svelte';
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import CopyButton from '../CopyButton.svelte';
  import Dialog from '../Dialog.svelte';
  import Field from '../Field.svelte';
  import Input from '../Input.svelte';
  import Panel from '../Panel.svelte';
  import Select from '../Select.svelte';
  import { api, type CreatedInvite, type GuestEntry, type GuestInvite, type GuestPermission, type GuestsStatus } from '../../lib/api';
  import { failed, notify } from '../../lib/feedback.svelte';
  import { poll } from '../../lib/format';
  import {
    EXPIRIES,
    PERMISSIONS,
    UNITS,
    expiresIn,
    guestLink,
    lifetime,
    linkAddress,
    permissionLabel,
    validAddress,
    type Expiry,
  } from '../../lib/guests';

  let status = $state.raw<GuestsStatus | null>(null);
  let loadError = $state('');
  let now = $state(Date.now() / 1000);

  let open = $state(false);
  // Strings, as the selects bind them.
  let permission = $state<string>('controller');
  let expiry = $state<string>('24h');
  let amount = $state<string | number | null>(2);
  let unit = $state('3600');
  let creating = $state(false);
  let created = $state.raw<CreatedInvite | null>(null);
  let address = $state('');
  /** The address the link starts with, so only one the owner typed is remembered. */
  let suggested = '';

  const seconds = $derived(lifetime(expiry as Expiry, Number(amount), Number(unit)));
  const addressValid = $derived(validAddress(address));
  const link = $derived(created && addressValid ? guestLink(address, created.token) : '');
  const guests = $derived(status?.guests ?? []);
  const invites = $derived(status?.invites ?? []);

  const ADDRESS_KEY = 'butterpollo.guest-address';
  // A browser that blocks site data throws on localStorage: the address then lasts for this visit.
  function savedAddress(): string | null {
    try {
      return localStorage.getItem(ADDRESS_KEY);
    } catch {
      return null;
    }
  }
  function rememberAddress() {
    try {
      if (address.trim() && address.trim() !== suggested) localStorage.setItem(ADDRESS_KEY, address.trim());
      else localStorage.removeItem(ADDRESS_KEY);
    } catch {
      // Not remembered; see savedAddress().
    }
  }

  async function load() {
    try {
      status = await api.guests.status();
      now = Date.now() / 1000;
      loadError = '';
    } catch (cause) {
      loadError = cause instanceof Error ? cause.message : String(cause);
    }
  }

  function invite() {
    created = null;
    open = true;
  }

  async function create() {
    if (seconds === null || creating) return;
    creating = true;
    try {
      const made = await api.guests.invite(permission as GuestPermission, seconds);
      suggested = linkAddress(location, made.addresses);
      address = savedAddress() ?? suggested;
      created = made;
      void load();
    } catch (cause) {
      failed('Creating the link failed', cause);
    } finally {
      creating = false;
    }
  }

  function done() {
    if (created) rememberAddress();
    open = false;
  }

  async function kick(guest: GuestEntry) {
    try {
      await api.guests.kick(guest.id);
      notify(`${guest.name} left the stream.`, 'ok');
    } catch (cause) {
      failed(`Removing ${guest.name} failed`, cause);
    }
    void load();
  }

  async function revoke(invite: GuestInvite) {
    try {
      await api.guests.revoke(invite.id);
      notify('Link revoked.', 'ok');
    } catch (cause) {
      failed('Revoking the link failed', cause);
    }
    void load();
  }

  const joinedWith = (invite: GuestInvite) => guests.filter((g) => g.invite === invite.id).length;

  onMount(() => poll(load, 3000));
</script>

<Panel title="Guests" flush>
  {#snippet actions()}
    <Button variant="primary" size="sm" icon="link" onclick={invite}>Invite</Button>
  {/snippet}
  {#if loadError && !status}
    <p class="notice danger row" role="alert">{loadError}</p>
  {:else if !guests.length && !invites.length}
    <p class="muted row">No guests</p>
  {:else}
    <ul class="list" aria-label="Guests and links">
      {#each guests as guest (guest.id)}
        <li>
          <span class="name">{guest.name}</span>
          <Badge tone="live">Watching</Badge>
          <Badge>{permissionLabel(guest.permission)}</Badge>
          <Button size="sm" variant="secondary" onclick={() => kick(guest)}>Kick<span class="visually-hidden"> {guest.name}</span></Button>
        </li>
      {/each}
      {#each invites as invite (invite.id)}
        {@const joined = joinedWith(invite)}
        <li>
          <span class="name">Link</span>
          <Badge>{permissionLabel(invite.permission)}</Badge>
          <span class="muted when">Expires {expiresIn(invite.expires, now)}{joined ? ` · ${joined} watching` : ''}</span>
          <Button size="sm" variant="danger" onclick={() => revoke(invite)}>Revoke</Button>
        </li>
      {/each}
    </ul>
  {/if}
</Panel>

<Dialog bind:open title={created ? 'Invite link' : 'Invite a guest'} dismissible={!creating} onclose={() => created && rememberAddress()}>
  {#if created}
    <div class="form">
      <Field label="Address" id="guest-address" error={addressValid ? undefined : 'A host name or IP address, with an optional port'}>
        <Input id="guest-address" bind:value={address} mono invalid={!addressValid} spellcheck="false" autocomplete="off" />
      </Field>
      <Field label="Link" id="guest-link">
        <div class="copy">
          <Input id="guest-link" value={link} mono readonly />
          <CopyButton value={link} label="Copy link" what="the invite link" showLabel />
        </div>
      </Field>
      <p class="muted when">{permissionLabel(created.invite.permission)} · expires {expiresIn(created.invite.expires)}</p>
    </div>
  {:else}
    <div class="form">
      <Field label="Guests can" id="guest-permission">
        <Select id="guest-permission" bind:value={permission} options={[...PERMISSIONS]} />
      </Field>
      <Field label="Link expires after" id="guest-expiry">
        <Select id="guest-expiry" bind:value={expiry} options={[...EXPIRIES]} />
      </Field>
      {#if expiry === 'custom'}
        <div class="custom">
          <Field label="Amount" id="guest-amount" error={seconds === null ? '5 minutes to 7 days' : undefined}>
            <Input id="guest-amount" type="number" min="1" step="1" bind:value={amount} invalid={seconds === null} />
          </Field>
          <Field label="Unit" id="guest-unit">
            <Select id="guest-unit" bind:value={unit} options={[...UNITS]} />
          </Field>
        </div>
      {/if}
    </div>
  {/if}
  {#snippet actions()}
    {#if created}
      <Button variant="primary" onclick={done}>Done</Button>
    {:else}
      <Button variant="secondary" disabled={creating} onclick={() => (open = false)}>Cancel</Button>
      <Button variant="primary" icon="link" busy={creating} disabled={seconds === null} onclick={create}>Create link</Button>
    {/if}
  {/snippet}
</Dialog>

<style>
  .row {
    margin: 0;
    padding: var(--space-3) var(--space-4);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .list li {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border-top: 1px solid var(--line);
  }
  .list li:first-child {
    border-top: 0;
  }
  .name {
    flex: 1;
    min-width: 0;
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }
  .when {
    font-size: var(--text-sm);
  }
  .form {
    display: grid;
    gap: var(--space-4);
  }
  .form p {
    margin: 0;
  }
  .custom {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-3);
  }
  .copy {
    display: flex;
    gap: var(--space-2);
    align-items: center;
  }
  .copy :global(input) {
    flex: 1;
    min-width: 0;
  }
</style>
