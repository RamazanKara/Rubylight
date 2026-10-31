<script lang="ts">
  import { onMount } from 'svelte';
  import Icon from '../components/Icon.svelte';
  import { refreshMetadata } from '../lib/session.svelte';
  import Badge from '../components/Badge.svelte';
  import StatusPill from '../components/StatusPill.svelte';
  import Button from '../components/Button.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import ClientAgent from '../components/devices/ClientAgent.svelte';
  import DeviceEditor from '../components/devices/DeviceEditor.svelte';
  import PairPanel from '../components/devices/PairPanel.svelte';
  import UsbDevices from '../components/devices/UsbDevices.svelte';
  import { showUsb } from '../components/devices/usb';
  import { grantedLabels } from '../components/devices/device';
  import { accessLabel, accessOf } from '../components/devices/access';
  import { api, type Client, type UsbAttachment } from '../lib/api';
  import { confirm, failed, notify } from '../lib/feedback.svelte';
  import { poll } from '../lib/format';
  import { guardNavigation } from '../lib/router.svelte';

  let clients = $state<Client[] | null>(null);
  let loadError = $state('');
  /** Why this PC offers no Sleep PC; null when it does. */
  let sleepUnavailable = $state<string | null>(null);
  let editing = $state<string | null>(null);
  let editorDirty = $state(false);
  let editorSaving = $state(false);
  /** `action:uuid` of the request in flight. */
  let busy = $state('');
  const acting = $derived(busy !== '' || editorSaving);
  let latestLoad = 0;
  let knownDevices: Set<string> | null = null;
  let usbDevices = $state<UsbAttachment[]>([]);
  let usbError = $state('');
  /** The USB attachment being detached. */
  let usbBusy = $state<number | null>(null);
  let latestUsbLoad = 0;

  // Its own request and error, so a host without USB forwarding still lists its devices.
  async function loadUsb() {
    const id = ++latestUsbLoad;
    try {
      const result = await api.usb.list();
      if (id !== latestUsbLoad) return;
      usbDevices = result.devices;
      usbError = '';
    } catch (error) {
      if (id === latestUsbLoad) usbError = error instanceof Error ? error.message : String(error);
    }
  }

  async function detachUsb(device: UsbAttachment) {
    if (usbBusy !== null) return;
    usbBusy = device.id;
    try {
      await api.usb.detach(device.id);
      notify(`Detached ${device.name} from ${device.client}.`, 'ok');
    } catch (error) {
      failed(`Detaching ${device.name} failed`, error);
    } finally {
      try {
        await loadUsb();
      } finally {
        usbBusy = null;
      }
    }
  }

  async function load() {
    // A poll that started before a save must not put the old values back.
    const id = ++latestLoad;
    try {
      const result = await api.clients.list();
      const list = result.clients;
      if (id !== latestLoad) return;
      if (knownDevices) {
        for (const client of list) {
          if (!knownDevices.has(client.uuid)) {
            notify(`${client.name} is paired.`, 'ok');
            void refreshMetadata().catch(() => {});
          }
        }
      }
      knownDevices = new Set(list.map(client => client.uuid));
      clients = list;
      sleepUnavailable = result.sleep_unavailable ?? null;
      loadError = '';
      if (editing && !list.some((client) => client.uuid === editing)) closeEditor();
    } catch (error) {
      if (id === latestLoad) loadError = error instanceof Error ? error.message : String(error);
    }
  }

  onMount(() => poll(() => Promise.all([load(), loadUsb()]), 5000));

  $effect(() => {
    if (!editorDirty) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', warn);
    const unguard = guardNavigation(() => confirm({
      title: 'Discard changes?',
      message: 'The changes you made to this device are not saved.',
      confirm: 'Discard',
    }));
    return () => {
      window.removeEventListener('beforeunload', warn);
      unguard();
    };
  });

  function closeEditor() {
    editing = null;
    editorDirty = false;
  }

  async function toggleEditor(uuid: string) {
    if (acting) return;
    if (
      editorDirty &&
      !(await confirm({
        title: 'Discard changes?',
        message: 'The changes you made to this device are not saved.',
        confirm: 'Discard',
      }))
    )
      return;
    const open = editing !== uuid;
    closeEditor();
    if (open) editing = uuid;
  }

  async function run(key: string, action: () => Promise<unknown>, success: string, failure: string) {
    if (acting) return;
    busy = key;
    try {
      await action();
      notify(success, 'ok');
    } catch (error) {
      failed(failure, error);
    } finally {
      // The list catches up before the actions unlock; the request timeout
      // bounds that wait, so a host that stops answering cannot keep them locked.
      try {
        await load();
      } finally {
        busy = '';
      }
    }
  }

  async function disconnect(client: Client) {
    if (acting) return;
    const ok = await confirm({
      title: `Disconnect ${client.name}?`,
      message: 'Its stream ends now. The device stays paired and can connect again.',
      confirm: 'Disconnect',
    });
    if (ok) {
      await run(
        `disconnect:${client.uuid}`,
        () => api.clients.disconnect(client.uuid),
        `Disconnected ${client.name}.`,
        'Disconnecting failed',
      );
    }
  }

  async function unpair(client: Client) {
    if (acting) return;
    const ok = await confirm({
      title: `Unpair ${client.name}?`,
      message: 'It is disconnected and removed. The device must pair again before it can stream.',
      confirm: 'Unpair',
      danger: true,
    });
    if (ok) {
      await run(`unpair:${client.uuid}`, () => api.clients.unpair(client.uuid), `Unpaired ${client.name}.`, 'Unpairing failed');
    }
  }

  async function unpairAll() {
    if (acting) return;
    const count = clients?.length ?? 0;
    const ok = await confirm({
      title: 'Unpair all devices?',
      message: `${count === 1 ? 'The paired device is' : `All ${count} paired devices are`} disconnected and removed. Each must pair again before it can stream.`,
      confirm: 'Unpair all',
      danger: true,
    });
    if (ok) {
      closeEditor();
      await run('unpair-all', () => api.clients.unpairAll(), 'Unpaired all devices.', 'Unpairing failed');
    }
  }

  /** The device's initial: its name says more than a guessed device kind. */
  const initial = (name: string) => [...name.trim()][0]?.toLocaleUpperCase() ?? '?';

  /** A row menu closes on a click outside it and on Escape, like other menus. */
  function menu(node: HTMLDetailsElement) {
    const outside = (event: MouseEvent) => {
      if (node.open && !node.contains(event.target as Node)) node.open = false;
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || !node.open) return;
      node.open = false;
      node.querySelector('summary')?.focus();
    };
    document.addEventListener('click', outside);
    node.addEventListener('keydown', escape);
    return {
      destroy() {
        document.removeEventListener('click', outside);
        node.removeEventListener('keydown', escape);
      },
    };
  }

  async function resetNetworkHistory(client: Client) {
    await run(
      `network-reset:${client.uuid}`,
      () => api.clients.resetNetworkHistory(client.uuid),
      `Network history reset for ${client.name}. Its next stream will test the normal pacing rate.`,
      'Resetting network history failed',
    );
  }
</script>

{#snippet unpairAllAction()}
<details class="more" use:menu><summary>Manage<Icon name="chevron" size={14} /></summary><div class="menu-list"><Button variant="danger" size="sm" disabled={acting} busy={busy === 'unpair-all'} onclick={unpairAll}>Unpair all</Button></div></details>
{/snippet}

<PageHeader title="Devices" subtitle="Connect a device, then choose what it can do." />

<PairPanel />

<Panel
  title="Paired devices"
  description={clients?.length ? `${clients.length} ${clients.length === 1 ? 'device' : 'devices'}` : undefined}
  actions={clients?.length ? unpairAllAction : undefined}
  flush
>

  {#if loadError}
    <div class="load-error notice danger" role="alert">
      <span>{clients ? 'Could not refresh the list' : 'Could not load devices'}: {loadError}</span>
      <Button size="sm" onclick={load}>Retry</Button>
    </div>
  {/if}

  {#if clients === null}
    {#if !loadError}<p class="placeholder muted">Loading devices…</p>{/if}
  {:else if clients.length === 0}
    <EmptyState icon="devices" title="No paired devices">
      <p>Pair a device above. It shows up here once pairing finishes.</p>
    </EmptyState>
  {:else}
    <ul class="devices">
      {#each clients as client (client.uuid)}
        {@const open = editing === client.uuid}
        <li class:open>
          <div class="device">
            <span class="avatar" aria-hidden="true">{initial(client.name)}</span>
            <div class="identity">
              <div class="title">
                <span class="name">{client.name}</span>
                {#if !client.enabled}<StatusPill status="idle" label="Disabled" />
                {:else if client.connected}<StatusPill status="ready" label="Connected" />
                {:else}<StatusPill status="offline" />{/if}
                {#if client.webrtc}<Badge>WebRTC</Badge>{/if}
              </div>
              <div class="meta muted">
                <span title={grantedLabels(client.perm) || 'No permissions'}>Access: {accessLabel(client.perm)}</span>
              </div>
              {#if accessOf(client.perm) === 'view'}<p class="network-memory muted">Can watch streams, but cannot start games or use controls.</p>{/if}
              {#if client.network_loss_routes?.length}
                <p class="network-memory muted">
                  Recent video loss: future streams at {[...new Set(client.network_loss_routes.map((route) => route.address))].join(', ')} start with smaller packet bursts.
                  Expires within 24 hours; the normal rate is tested every second stream.
                </p>
              {/if}
              <ClientAgent {client} />
            </div>
            <div class="actions">
              <Button size="sm" disabled={acting} icon={open ? 'x' : accessOf(client.perm) === 'view' ? 'key' : 'settings'} onclick={() => toggleEditor(client.uuid)}>
                {open ? 'Close' : accessOf(client.perm) === 'view' ? 'Change access' : 'Settings'}<span class="visually-hidden"> {client.name}</span>
              </Button>
              <details class="more" use:menu><summary>More<span class="visually-hidden"> for {client.name}</span><Icon name="chevron" size={14} /></summary><div class="menu-list">
              {#if client.network_loss_routes?.length}
                <Button size="sm" disabled={acting} busy={busy === `network-reset:${client.uuid}`} onclick={() => resetNetworkHistory(client)}>
                  Reset network history<span class="visually-hidden"> for {client.name}</span>
                </Button>
              {/if}
              {#if client.connected}
                <Button size="sm" disabled={acting} busy={busy === `disconnect:${client.uuid}`} onclick={() => disconnect(client)}>
                  Disconnect<span class="visually-hidden"> {client.name}</span>
                </Button>
              {/if}

              <Button
                size="sm"
                variant="danger"
                disabled={acting}
                busy={busy === `unpair:${client.uuid}`}
                onclick={() => unpair(client)}
              >
                Unpair<span class="visually-hidden"> {client.name}</span>
              </Button>

              </div></details>
            </div>
          </div>
          {#if open}
            <DeviceEditor {client} {sleepUnavailable} bind:dirty={editorDirty} bind:saving={editorSaving} disabled={busy !== ''} onsaved={load} />
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</Panel>

{#if usbError || showUsb(clients, usbDevices)}
  <UsbDevices devices={usbDevices} error={usbError} busy={usbBusy} ondetach={detachUsb} />
{/if}

<style>
  /* Row menus: a quiet button that opens a small list over the page. */
  .more { position: relative; }
  .more > summary {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    min-height: var(--control-height);
    padding: 0 var(--space-3);
    border-radius: var(--radius);
    color: var(--ink-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    list-style: none;
    transition: background var(--duration-fast);
  }
  .more > summary::-webkit-details-marker { display: none; }
  .more > summary::before,
  .more > summary::after { content: none; display: none; }
  .more > summary :global(svg) { transform: rotate(90deg); transition: transform var(--duration-fast) var(--ease-standard); }
  .more[open] > summary :global(svg) { transform: rotate(-90deg); }
  .more > summary:hover,
  .more[open] > summary { background: var(--sunken); color: var(--ink); }
  .menu-list {
    position: absolute;
    right: 0;
    top: calc(100% + var(--space-1));
    z-index: var(--z-sticky);
    display: grid;
    gap: var(--space-2);
    min-width: 200px;
    padding: var(--space-2);
    background: var(--panel-raised);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    box-shadow: var(--shadow-pop);
  }
  /* A menu reaches past the list's rounded clip while it is open. */
  .devices:has(.more[open]) { overflow: visible; }
  .avatar {
    flex: none;
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: var(--radius);
    background: var(--sunken);
    border: 1px solid var(--line);
    color: var(--ink-2);
    font-family: var(--font-display);
    font-size: var(--text-md);
    font-weight: var(--weight-bold);
  }
  .open .avatar { background: var(--panel); }
  .network-memory {
    margin: var(--space-1) 0 0;
    font-size: var(--text-sm);
    max-width: 65ch;
  }
  .placeholder {
    padding: var(--space-5);
    font-size: var(--text-sm);
  }
  .load-error {
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    margin: var(--space-3) var(--space-5);
  }
  .devices {
    margin: 0;
    padding: 0;
    list-style: none;
    /* clip, not hidden, so the editor's sticky footer still sticks. */
    overflow: clip;
    border-radius: 0 0 calc(var(--radius-lg) - 1px) calc(var(--radius-lg) - 1px);
  }
  .devices > li + li {
    border-top: 1px solid var(--line);
  }
  .device {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3) var(--space-4);
    flex-wrap: wrap;
    padding: var(--space-4) var(--space-5);
  }
  .open .device {
    background: var(--sunken);
  }
  .identity {
    display: grid;
    gap: 2px;
    flex: 1 1 220px;
    min-width: 0;
  }
  .title {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .name {
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }
  .meta {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    font-size: var(--text-sm);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-left: auto;
  }
  @media (max-width: 520px) {
    .device {
      padding: var(--space-4);
      align-items: flex-start;
    }
    /* Under the name, lined up with it rather than with the avatar. */
    .actions {
      flex: 1 0 100%;
      padding-left: calc(40px + var(--space-4));
      margin-left: 0;
    }
    .load-error {
      margin: var(--space-3) var(--space-4);
    }
  }
</style>
