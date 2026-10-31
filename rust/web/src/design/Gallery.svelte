<script lang="ts">
  // Every core component in every state, for design review and screenshots.
  import Badge from '../components/Badge.svelte';
  import Banner from '../components/Banner.svelte';
  import Button from '../components/Button.svelte';
  import Card from '../components/Card.svelte';
  import Dialog from '../components/Dialog.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import Field from '../components/Field.svelte';
  import Input from '../components/Input.svelte';
  import Panel from '../components/Panel.svelte';
  import Select from '../components/Select.svelte';
  import Skeleton from '../components/Skeleton.svelte';
  import StatusPill, { statusLabels, type Status } from '../components/StatusPill.svelte';
  import Table from '../components/Table.svelte';
  import Tabs from '../components/Tabs.svelte';
  import Toggle from '../components/Toggle.svelte';
  import { applyTheme, storedTheme, type Theme } from '../lib/theme';

  let theme = $state<Theme>(storedTheme());
  $effect(() => applyTheme(theme));

  let name = $state('Gaming PC');
  let port = $state('47989x');
  let codec = $state('av1');
  let pairing = $state(true);
  let wake = $state(false);
  let dialogOpen = $state(false);
  let tab = $state('video');

  const swatches = ['bg', 'panel', 'sunken', 'ink', 'ink-2', 'muted', 'accent', 'accent-text', 'accent-soft',
    'ok', 'ok-soft', 'warn', 'warn-soft', 'danger', 'danger-soft', 'focus', 'line', 'line-strong'];
  const sizes = ['2xl', 'xl', 'lg', 'md', '', 'sm', 'xs'];
</script>

<main class="gallery stack">
  <header class="row top">
    <h1>Design system</h1>
    <Select
      aria-label="Theme"
      bind:value={theme}
      options={[{ value: 'system', label: 'System theme' }, { value: 'light', label: 'Light' }, { value: 'dark', label: 'Dark' }]}
    />
  </header>

  <Panel title="Colour">
    <div class="swatches">
      {#each swatches as swatch (swatch)}
        <div class="swatch"><span style:background="var(--{swatch})"></span><code>--{swatch}</code></div>
      {/each}
    </div>
  </Panel>

  <Panel title="Type">
    <div class="stack type">
      {#each sizes as size (size)}
        <p style:font-size="var(--text{size ? `-${size}` : ''})">--text{size ? `-${size}` : ''} Stream your games</p>
      {/each}
    </div>
  </Panel>

  <Panel title="Buttons and status">
    <div class="stack">
      <div class="row">
        <Button variant="primary" icon="play">Start stream</Button>
        <Button icon="refresh">Recheck</Button>
        <Button variant="ghost">Cancel</Button>
        <Button variant="danger" icon="trash">Unpair</Button>
        <Button busy>Saving</Button>
        <Button disabled>Disabled</Button>
        <Button variant="primary" disabled>Disabled primary</Button>
        <Button size="sm">Small</Button>
      </div>
      <div class="row">
        {#each Object.keys(statusLabels) as status (status)}<StatusPill status={status as Status} />{/each}
      </div>
      <div class="row">
        <Badge>HEVC</Badge><Badge tone="ok">Paired</Badge><Badge tone="warn">Applies after a restart</Badge>
        <Badge tone="danger">Blocked</Badge><Badge tone="accent">New</Badge><Badge tone="live">Live</Badge>
      </div>
    </div>
  </Panel>

  <Panel title="Banners">
    <div class="stack">
      <Banner title="Pairing is on">New devices can pair with this PC until you turn it off.</Banner>
      <Banner tone="ok" title="Encoder ready">H.264, HEVC, AV1 and PyroWave.</Banner>
      <Banner tone="warn" title="No audio output" ondismiss={() => {}}>
        Streams have no sound until a speaker or headset is on.
        {#snippet actions()}<Button size="sm">Open sound settings</Button>{/snippet}
      </Banner>
      <Banner tone="danger" title="Capture failed">The display was lost while streaming.
        {#snippet actions()}<Button size="sm" variant="primary">Restart capture</Button><Button size="sm" variant="ghost">View log</Button>{/snippet}
      </Banner>
    </div>
  </Panel>

  <Panel title="Forms">
    <div class="form-grid">
      <Field label="Host name" id="g-name" hint="Shown in Moonlight and Nightjar.">
        <Input id="g-name" bind:value={name} />
      </Field>
      <Field label="Port" id="g-port" error="Use a number from 1024 to 65535.">
        <Input id="g-port" bind:value={port} invalid mono describedby="g-port-error" />
      </Field>
      <Field label="Codec" id="g-codec" hint="Offered when the encoder supports it.">
        <Select id="g-codec" bind:value={codec} options={[
          { value: 'h264', label: 'H.264' }, { value: 'hevc', label: 'HEVC' }, { value: 'av1', label: 'AV1' },
        ]} />
      </Field>
      <div class="stack tight">
        <Toggle label="Allow pairing" hint="Turn it off once your devices are paired." bind:checked={pairing} />
        <Toggle label="Wake on LAN" bind:checked={wake} />
        <Toggle label="Unavailable" disabled />
      </div>
    </div>
  </Panel>

  <Panel title="Tabs, dialog and table">
    <div class="stack">
      <Tabs label="Stream settings" bind:selected={tab} tabs={[
        { id: 'video', label: 'Video' }, { id: 'audio', label: 'Audio', badge: 2 },
        { id: 'input', label: 'Input' }, { id: 'usb', label: 'USB', disabled: true },
      ]}>
        {#snippet panel(id)}<p class="muted">The {id} panel. Arrow keys move between tabs.</p>{/snippet}
      </Tabs>
      <div><Button onclick={() => (dialogOpen = true)}>Open dialog</Button></div>
      <Table caption="Connected devices">
        <thead><tr><th>Device</th><th>State</th><th class="numeric">Bitrate</th></tr></thead>
        <tbody>
          <tr><td>Pixel 9</td><td><StatusPill status="streaming" /></td><td class="numeric num">48.2 Mbps</td></tr>
          <tr><td>Living room TV</td><td><StatusPill status="offline" /></td><td class="numeric num">-</td></tr>
        </tbody>
      </Table>
    </div>
  </Panel>

  <Panel title="Cards, empty and loading">
    <div class="cards">
      <Card href="#devices" label="Pixel 9 settings">
        <div class="row between"><h3>Pixel 9</h3><StatusPill status="streaming" /></div>
        <p class="muted">2400 x 1080 at 120 Hz</p>
        {#snippet footer()}<Badge>Files</Badge><Badge>Clipboard</Badge>{/snippet}
      </Card>
      <Card selected>
        <div class="row between"><h3>Living room TV</h3><StatusPill status="offline" /></div>
        <p class="muted">Last seen yesterday</p>
      </Card>
      <Card><Skeleton label="Loading device" lines={3} /></Card>
      <Card padding="none"><EmptyState icon="devices" title="No devices yet">Pair a phone, TV or PC to stream to it.</EmptyState></Card>
    </div>
  </Panel>
</main>

<Dialog bind:open={dialogOpen} title="Unpair Pixel 9?" description="It will need a new PIN to stream from this PC again.">
  {#snippet actions()}
    <Button onclick={() => (dialogOpen = false)}>Cancel</Button>
    <Button variant="danger" onclick={() => (dialogOpen = false)}>Unpair</Button>
  {/snippet}
</Dialog>

<style>
  .gallery {
    max-width: 1100px;
    margin: 0 auto;
    padding: var(--space-6) var(--space-5);
  }
  .top {
    justify-content: space-between;
  }
  .top :global(.select) {
    width: auto;
  }
  .swatches {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: var(--space-3);
  }
  .swatch {
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-xs);
  }
  .swatch span {
    height: 44px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
  }
  .type {
    gap: var(--space-2);
  }
  .tight {
    gap: 0;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
    gap: var(--space-4);
  }
  .between {
    justify-content: space-between;
  }
</style>
