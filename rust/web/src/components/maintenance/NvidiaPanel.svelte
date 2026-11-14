<script lang="ts">
  // The NVIDIA components: Rubylight's NVIDIA adapter and NVIDIA's RTX Video
  // files, which RTX HDR and RTX Video enhancement need. The host downloads
  // them from its own release at the user's request and checks every file.
  import { onDestroy, onMount } from 'svelte';
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import LoadState from '../LoadState.svelte';
  import { api, type NvidiaComponent } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { refreshMetadata, session } from '../../lib/session.svelte';
  import { reason } from './util';

  const ABOUT =
    'RTX HDR and RTX Video enhancement need them: Rubylight’s NVIDIA adapter and NVIDIA’s RTX Video files, downloaded from this version’s release.';

  let component = $state<NvidiaComponent | null>(null);
  let loadError = $state('');
  let busy = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  const busyStates = ['download', 'check', 'remove'];
  const working = $derived(!!component && busyStates.includes(component.state));
  const labels: Record<string, string> = {
    download: 'Downloading and checking the files…',
    check: 'Asking the graphics driver what it offers…',
    remove: 'Removing…',
  };
  const percent = $derived(
    component && component.total > 0 ? Math.min(100, Math.round((component.done / component.total) * 100)) : null,
  );
  const offered = $derived(session.metadata?.nvidia_enhancements);

  async function load() {
    clearTimeout(timer);
    const was = component?.state;
    try {
      component = await api.nvidiaComponent.status();
      loadError = '';
    } catch (error) {
      loadError = reason(error);
      return;
    }
    if (busyStates.includes(component.state)) {
      timer = setTimeout(load, 1000);
    } else if (was && busyStates.includes(was)) {
      if (component.state === 'ready' && was !== 'remove') notify('NVIDIA components installed.', 'ok');
      // The settings and the app editor show what the GPU and driver offer now.
      refreshMetadata().catch(() => {});
    }
  }

  async function install() {
    busy = true;
    try {
      await api.nvidiaComponent.install();
      await load();
    } catch (error) {
      failed('The NVIDIA components were not downloaded', error);
    } finally {
      busy = false;
    }
  }

  async function remove() {
    const ok = await confirm({
      title: 'Remove the NVIDIA components?',
      message: 'RTX HDR and RTX Video enhancement stop until you download them again.',
      confirm: 'Remove',
      danger: true,
    });
    if (!ok) return;
    busy = true;
    try {
      await api.nvidiaComponent.remove();
      notify('NVIDIA components removed.', 'ok');
      await load();
      await refreshMetadata().catch(() => {});
    } catch (error) {
      failed('The NVIDIA components were not removed', error);
    } finally {
      busy = false;
    }
  }

  onMount(load);
  onDestroy(() => clearTimeout(timer));
</script>

{#snippet feature(name: string, on: boolean | undefined, why: string | undefined)}
  <dt>{name}</dt>
  <dd>
    <Badge tone={on ? 'ok' : 'warn'}>{on ? 'Offered' : 'Not offered'}</Badge>
    {#if !on && why}<span class="muted why">{why}</span>{/if}
  </dd>
{/snippet}

<Panel title="NVIDIA components" level={3} description={ABOUT}>
  {#if component}
    <div class="stack">
      {#if working}
        <p class="lead" role="status">{labels[component.state]}</p>
        <div
          class="meter"
          role="progressbar"
          aria-label="NVIDIA components"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={component.state === 'download' ? (percent ?? undefined) : undefined}
          class:indeterminate={percent === null || component.state !== 'download'}
        >
          <span class="fill" style:transform="scaleX({(percent ?? 30) / 100})"></span>
        </div>
      {:else}
        <dl class="facts">
          <dt>Installed</dt>
          <dd>
            {#if component.installed && component.outdated}
              <Badge tone="warn">For an earlier Rubylight</Badge>
            {:else if component.installed}
              <Badge tone="ok">Yes</Badge>
            {:else if component.state === 'failed'}
              <Badge tone="warn">Failed</Badge>
            {:else}
              <Badge>No</Badge>
            {/if}
          </dd>
          {#if component.installed}
            {@render feature('RTX HDR', offered?.rtx_hdr, offered?.errors?.rtx_hdr)}
            {@render feature('RTX Video enhancement', offered?.rtx_video, offered?.errors?.rtx_video)}
          {/if}
        </dl>
        {#if component.error}
          <p class="error" role="alert">{component.error}</p>
        {/if}
        {#if component.installed && component.outdated && !component.error}
          <p class="muted hint">Rubylight fetches this version’s NVIDIA components on its own and keeps using the installed ones until then.</p>
        {/if}
        <div class="row">
          {#if component.state !== 'ready' || component.outdated}
            {#if component.nvidia_gpu}
              <Button {busy} onclick={install}>{component.state === 'failed' ? 'Try again' : 'Download'}</Button>
            {:else if !component.installed}
              <span class="muted hint">This PC has no NVIDIA graphics card.</span>
            {/if}
          {/if}
          {#if component.installed || component.state === 'failed'}
            <!-- What a failed check left installed goes too. -->
            <Button {busy} onclick={remove}>Remove</Button>
          {/if}
        </div>
      {/if}
    </div>
  {:else}
    <LoadState error={loadError} what="the NVIDIA components" onretry={load} />
  {/if}
</Panel>

<style>
  .hint,
  .why {
    font-size: var(--text-xs);
  }
  .why {
    margin-left: var(--space-2);
  }
  .lead {
    margin: 0;
  }
  .error {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-sm);
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
</style>
