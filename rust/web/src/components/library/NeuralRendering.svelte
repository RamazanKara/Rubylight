<script lang="ts">
  // The game page's Neural Rendering panel: the frame generation switch (the notice once, then
  // download, install and removal with progress) and the removal of Rubylight 3.0's upscaling. States come from lib/neural-view.ts; this file only draws them and runs the actions.
  import { onMount, untrack } from 'svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import LoadState from '../LoadState.svelte';
  import Panel from '../Panel.svelte';
  import PathPicker from '../PathPicker.svelte';
  import Toggle from '../Toggle.svelte';
  import { api, type NeuralStatus } from '../../lib/api';
  import { confirm, failed } from '../../lib/feedback.svelte';
  import { poll } from '../../lib/format';
  import { ABOUT, NOTICE, neuralHint, neuralView, pollsNeural, removesOnlyEarlier } from '../../lib/neural-view';
  import { statusLoader } from '../maintenance/status-loader';
  import { reason } from '../maintenance/util';

  let {
    uuid,
    initial = null,
  }: {
    /** The saved app's uuid. */
    uuid: string;
    /** A status to show before the first load (the tests render with one). */
    initial?: NeuralStatus | null;
  } = $props();

  const uid = $props.id();
  let status = $state<NeuralStatus | null>(untrack(() => initial));
  let loadError = $state('');
  let busy = $state(false);
  /** The program the user picked when the host could not find it. */
  let exe = $state('');
  /** Where the switch was put while its request runs; a cancelled notice puts it back. */
  let wanted = $state<boolean | null>(null);
  /** The host took a request whose first status has not arrived: keep asking until one does. */
  let pending = $state(false);

  const view = $derived(status ? neuralView(status) : null);

  const loader = statusLoader(
    () => api.neural.status(uuid),
    (value) => {
      status = value;
      loadError = '';
      pending = false;
    },
    (error) => {
      loadError = reason(error);
    },
  );
  const load = loader.load;

  async function turnOn(program?: string) {
    if (!status?.accepted) {
      if (!(await confirm(NOTICE))) return;
      try {
        await api.neural.accept();
      } catch (error) {
        failed('Frame generation failed', error);
        return;
      }
    }
    busy = true;
    try {
      await api.neural.enable(uuid, program);
      pending = true;
    } catch (error) {
      failed('Turning on frame generation failed', error);
    } finally {
      busy = false;
    }
    await load();
  }

  async function turnOff(earlier = false) {
    busy = true;
    try {
      await api.neural.disable(uuid, earlier);
      pending = true;
    } catch (error) {
      failed('Removing Neural Rendering failed', error);
    } finally {
      busy = false;
    }
    await load();
  }

  async function change(checked: boolean) {
    wanted = checked;
    try {
      await (checked ? turnOn() : turnOff());
    } finally {
      wanted = null;
    }
  }

  onMount(() => {
    if (!status) load();
    // Marked as a poll and paused while the tab is hidden, so a page left open lets the PC sleep.
    return poll(() => {
      if (((view && pollsNeural(view)) || pending) && !busy) return loader.tick();
    }, 1500);
  });
</script>

{#if loadError && !status}
  <Panel title="Neural Rendering">
    <LoadState error={loadError} what="Neural Rendering" onretry={load} />
  </Panel>
{:else if view}
  <Panel collapsible title="Neural Rendering" description={ABOUT}>
    <div class="neural">
      {#if view.kind === 'unavailable'}
        <Toggle label="Frame generation" checked={false} disabled hint={view.reason} />
      {:else if view.kind === 'working'}
        <p class="lead" role="status">{view.label}</p>
        <div
          class="meter"
          role="progressbar"
          aria-label="Neural Rendering"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={view.percent ?? undefined}
          class:indeterminate={view.percent === null}
        >
          <span class="fill" style:transform="scaleX({(view.percent ?? 30) / 100})"></span>
        </div>
      {:else}
        {@const on = view.kind === 'on' || (view.kind === 'failed' && view.on)}
        <Toggle
          label="Frame generation"
          hint={neuralHint(view, status?.gpu_note)}
          checked={wanted ?? on}
          disabled={busy}
          onchange={change}
        />
        {#if (view.kind === 'off' || view.kind === 'on') && view.kept.length > 0}
          <p class="muted note">
            Left in the game’s folder because they changed since they were installed, are not Rubylight’s or a program still held them open: {view.kept.join(', ')}.
          </p>
        {:else if view.kind === 'failed'}
          <Banner tone="danger" title={view.title} live>
            {view.error}
          </Banner>
        {:else if view.kind === 'choose'}
          <Banner tone="warn" title="Which program is the game?" live>{view.reason} Choose the game’s .exe.</Banner>
          <div class="choose">
            <PathPicker kind="executable" value={exe} onpick={(path) => (exe = path)}>
              <input
                id="{uid}-exe"
                class="input mono"
                aria-label="The game’s program"
                autocomplete="off"
                spellcheck="false"
                placeholder="C:\Games\Game\Game.exe"
                bind:value={exe}
              />
            </PathPicker>
            <Button variant="primary" busy={busy} disabled={!exe.trim()} onclick={() => turnOn(exe.trim())}>Install</Button>
          </div>
        {/if}
      {/if}
      {#if 'earlier' in view && view.earlier !== null}
        <div class="earlier">
          <p class="muted note">
            The earlier neural upscaling{view.earlier ? ` (release ${view.earlier})` : ''} is still in the game’s folder. Rubylight
            no longer offers it; remove it here.
          </p>
          <Button busy={busy} onclick={() => turnOff(removesOnlyEarlier(view))}>Remove</Button>
        </div>
      {/if}
      {#if loadError}<p class="muted note" role="alert">Could not refresh Neural Rendering: {loadError}</p>{/if}
    </div>
  </Panel>
{/if}

<style>
  .neural {
    display: grid;
    gap: var(--space-4);
    max-width: 68ch;
  }
  .lead {
    margin: 0;
    line-height: var(--leading);
  }
  .note {
    margin: 0;
    font-size: var(--text-xs);
    line-height: var(--leading-snug);
  }
  .earlier {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    gap: var(--space-3);
  }
  .choose {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: start;
    gap: var(--space-2) var(--space-3);
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
    animation: sweep var(--pulse-duration) var(--ease-standard) infinite;
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
