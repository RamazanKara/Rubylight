<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../components/Button.svelte';
  import CopyButton from '../components/CopyButton.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import Field from '../components/Field.svelte';
  import LoadState from '../components/LoadState.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import { api, type WebhookEntry, type WebhookList, type WebhookStatus } from '../lib/api';
  import { confirm, failed, notify } from '../lib/feedback.svelte';
  import { when } from '../lib/format';

  // What each event means, in the order the host lists them.
  const EVENT_LABELS: Record<string, string> = {
    'pairing.paired': 'A device paired',
    'pairing.unpaired': 'A device was removed',
    'app.started': 'An app started',
    'app.stopped': 'An app stopped',
    'session.started': 'A stream started',
    'session.ended': 'A stream ended',
  };

  const uid = $props.id();
  let list = $state<WebhookList | null>(null);
  let error = $state('');
  // The hook being edited (its saved URL), or '' for a new one.
  let editing = $state<string | null>(null);
  let url = $state('');
  let events = $state<string[]>([]);
  let busy = $state(false);
  let testing = $state<string | null>(null);
  // Shown once, right after the host made it.
  let secret = $state<{ url: string; value: string } | null>(null);

  const full = $derived(list !== null && list.hooks.length >= list.max);

  async function load() {
    try {
      list = await api.webhooks.list();
      error = '';
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
    }
  }

  function edit(hook: WebhookEntry | null) {
    editing = hook?.url ?? '';
    url = hook?.url ?? '';
    events = hook ? [...hook.events] : [];
  }

  function toggle(event: string, on: boolean) {
    events = on ? [...events, event] : events.filter(e => e !== event);
  }

  async function save(event: SubmitEvent, rotate = false) {
    event.preventDefault();
    if (editing === null) return;
    busy = true;
    try {
      const target = url.trim();
      const result = await api.webhooks.save({
        url: target,
        events,
        ...(editing ? { previous: editing } : {}),
        ...(rotate ? { rotate_secret: true } : {}),
      });
      if (result.secret) secret = { url: target, value: result.secret };
      notify(editing ? 'Webhook saved.' : 'Webhook added.', 'ok');
      editing = null;
      await load();
    } catch (failure) {
      failed('Saving the webhook failed', failure);
    } finally {
      busy = false;
    }
  }

  async function rotate(hook: WebhookEntry) {
    const ok = await confirm({
      title: 'Replace the secret?',
      message: 'The receiver must use the new secret to check signatures. Deliveries signed with the old one stop verifying.',
      confirm: 'Replace',
      danger: true,
    });
    if (!ok) return;
    busy = true;
    try {
      const result = await api.webhooks.save({ url: hook.url, events: hook.events, previous: hook.url, rotate_secret: true });
      if (result.secret) secret = { url: hook.url, value: result.secret };
    } catch (failure) {
      failed('Replacing the secret failed', failure);
    } finally {
      busy = false;
    }
  }

  async function remove(hook: WebhookEntry) {
    const ok = await confirm({
      title: 'Delete this webhook?',
      message: `${hook.url} stops getting events.`,
      confirm: 'Delete',
      danger: true,
    });
    if (!ok) return;
    busy = true;
    try {
      await api.webhooks.remove(hook.url);
      if (secret?.url === hook.url) secret = null;
      notify('Webhook deleted.', 'ok');
      await load();
    } catch (failure) {
      failed('Deleting the webhook failed', failure);
    } finally {
      busy = false;
    }
  }

  async function test(hook: WebhookEntry) {
    testing = hook.url;
    try {
      const { result } = await api.webhooks.test(hook.url);
      if (result.ok) notify('The receiver accepted the test event.', 'ok');
      else failed('The test event failed', new Error(describe(result)));
      await load();
    } catch (failure) {
      failed('Sending the test event failed', failure);
    } finally {
      testing = null;
    }
  }

  function describe(status: WebhookStatus) {
    if (status.ok) return `Delivered${status.http_status ? ` (HTTP ${status.http_status})` : ''}`;
    const tries = status.attempts > 1 ? ` after ${status.attempts} tries` : '';
    return `${status.error ?? 'Failed'}${tries}`;
  }

  onMount(load);
</script>

<div class="page">
  <PageHeader
    title="Webhooks"
    subtitle="Tell other services when devices pair, apps start and stop, and streams begin and end."
  />
  <div class="stack">
    <p class="notice usage">
      <span>Each event is a signed JSON POST. Check the <code class="name">X-Rubylight-Signature</code> header, an HMAC-SHA256 of the body with the hook's secret, and refuse deliveries older than 5 minutes.</span>
    </p>

    {#if secret}
      <div class="notice secret" role="status">
        <p><strong>Copy this secret now.</strong> It is shown only once; the receiver for <span class="mono">{secret.url}</span> needs it to check signatures.</p>
        <div class="value"><code class="mono">{secret.value}</code><CopyButton value={secret.value} label="Copy secret" what="the secret" showLabel /></div>
        <div><Button size="sm" onclick={() => (secret = null)}>Done</Button></div>
      </div>
    {/if}

    <Panel title="Webhooks" description={list ? `${list.hooks.length} of ${list.max}` : undefined} flush>
      {#if list?.error}
        <p class="notice pad">The saved webhooks could not be read: {list.error}</p>
      {/if}
      {#if list === null || (error && list)}
        <div class="pad"><LoadState {error} what="the webhooks" stale={list !== null} onretry={load} /></div>
      {/if}
      {#if list && list.hooks.length === 0 && !list.error}
        <EmptyState icon="link" title="No webhooks">
          <p>Add one below to send events to a URL.</p>
        </EmptyState>
      {:else if list}
        <ul>
          {#each list.hooks as hook (hook.url)}
            <li>
              <div class="info">
                <strong class="mono url">{hook.url}</strong>
                <p class="muted">{hook.events.map(e => EVENT_LABELS[e] ?? e).join(', ')}</p>
                {#if hook.last}
                  <p class:failed={!hook.last.ok}>
                    Last: {hook.last.event}, {when(hook.last.time_unix_ms / 1000)}: {describe(hook.last)}
                  </p>
                {:else}
                  <p class="muted">Nothing sent since the host started.</p>
                {/if}
              </div>
              <div class="actions">
                <Button size="sm" busy={testing === hook.url} disabled={testing !== null} onclick={() => test(hook)}>Send test</Button>
                <Button size="sm" disabled={busy} onclick={() => edit(hook)}>Edit</Button>
                <Button size="sm" disabled={busy} onclick={() => rotate(hook)}>New secret</Button>
                <Button size="sm" variant="danger" disabled={busy} onclick={() => remove(hook)}>Delete</Button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </Panel>

    {#if editing !== null && list}
      <Panel title={editing ? 'Edit webhook' : 'Add a webhook'}>
        <form class="form" onsubmit={save}>
          <Field label="URL" id="{uid}-url" hint="http:// or https://. A token in the path or query stays out of the host log.">
            <input id="{uid}-url" class="input mono" type="url" required autocomplete="off" spellcheck="false" bind:value={url} />
          </Field>
          <fieldset>
            <legend>Events</legend>
            {#each list.events as name (name)}
              <label class="event">
                <input type="checkbox" checked={events.includes(name)} onchange={e => toggle(name, e.currentTarget.checked)} />
                <span>{EVENT_LABELS[name] ?? name}</span> <span class="muted mono">{name}</span>
              </label>
            {/each}
          </fieldset>
          <div class="row">
            <Button type="submit" variant="primary" {busy} disabled={!url.trim() || events.length === 0}>{editing ? 'Save' : 'Add webhook'}</Button>
            <Button onclick={() => (editing = null)}>Cancel</Button>
          </div>
        </form>
      </Panel>
    {:else if list}
      <div><Button icon="plus" disabled={full || Boolean(list.error)} onclick={() => edit(null)}>Add a webhook</Button></div>
    {/if}
  </div>
</div>

<style>
  .page {
    display: grid;
    gap: var(--space-5);
    min-width: 0;
  }
  .stack {
    gap: var(--space-5);
  }
  .usage,
  .url {
    overflow-wrap: anywhere;
  }
  code {
    padding: 1px 6px;
    border: 1px solid var(--line);
    border-radius: 4px;
    background: var(--panel);
    white-space: normal;
    overflow-wrap: anywhere;
  }
  /* A header name stays whole instead of breaking at its hyphen. */
  code.name {
    white-space: nowrap;
  }
  .secret {
    display: grid;
    gap: var(--space-3);
  }
  .value {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-3);
  }
  .pad {
    margin: var(--space-4) var(--space-5);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: var(--space-4);
    padding: var(--space-3) var(--space-5);
    border-bottom: 1px solid var(--line);
  }
  li:last-child {
    border-bottom: 0;
  }
  /* Rows line up with the panel title, which moves in on phones. */
  @media (max-width: 520px) {
    li {
      padding-inline: var(--space-4);
    }
    .pad {
      margin-inline: var(--space-4);
    }
  }
  .info {
    display: grid;
    gap: var(--space-1);
    min-width: 0;
    font-size: var(--text-sm);
  }
  .info strong {
    font-size: var(--text-base, inherit);
  }
  .failed {
    color: var(--danger, inherit);
  }
  .actions,
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .form {
    display: grid;
    gap: var(--space-4);
  }
  fieldset {
    display: grid;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
    border: 0;
  }
  legend {
    margin-bottom: var(--space-2);
    font-weight: var(--weight-semibold);
  }
  .event {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
</style>
