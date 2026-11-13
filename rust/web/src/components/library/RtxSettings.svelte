<script lang="ts">
  // The app's NVIDIA image enhancements: RTX HDR and RTX Video enhancement.
  // Shown only by Rubylight's NVIDIA build; a feature this PC does not offer
  // says why instead of showing its controls.
  import Banner from '../Banner.svelte';
  import Field from '../Field.svelte';
  import Toggle from '../Toggle.svelte';
  import NumberField from './NumberField.svelte';
  import type { NvidiaEnhancements } from '../../lib/api';
  import type { AppDraft } from './draft.svelte';

  let { draft, offered }: { draft: AppDraft; offered: NvidiaEnhancements } = $props();

  const uid = $props.id();
</script>

<div class="rtx">
  <section aria-labelledby="{uid}-hdr">
    <h3 id="{uid}-hdr">RTX HDR</h3>
    {#if offered.rtx_hdr}
      <Toggle
        label="Use RTX HDR for this app"
        hint="Turns the app’s SDR picture into HDR when a device streams in HDR."
        bind:checked={() => draft.flag('rtx-hdr'), (value) => draft.setFlag('rtx-hdr', value)}
      />
      <div class="numbers">
        <NumberField {draft} key="rtx-hdr-sdr-brightness" label="SDR brightness" hint="0 to 100." placeholder="Host setting" />
        <NumberField {draft} key="rtx-hdr-contrast" label="Contrast" hint="−100 to 100." placeholder="Host setting" />
        <NumberField {draft} key="rtx-hdr-saturation" label="Saturation" hint="−100 to 100." placeholder="Host setting" />
        <NumberField {draft} key="rtx-hdr-middle-gray" label="Middle gray" hint="10 to 100." placeholder="Host setting" />
      </div>
    {:else}
      <Banner tone="danger" title="RTX HDR is unavailable on this PC">
        {offered.errors.rtx_hdr ?? 'The NVIDIA graphics card or driver does not offer it.'}
      </Banner>
    {/if}
  </section>

  <section aria-labelledby="{uid}-video">
    <h3 id="{uid}-video">RTX Video enhancement</h3>
    {#if offered.rtx_video}
      <Field label="RTX Video enhancement" id="{uid}-video-on" hint="Cleans up and sharpens the picture before it is encoded.">
        <select
          id="{uid}-video-on"
          class="select"
          bind:value={() => draft.tristate('rtx-video'), (value) => draft.setTristate('rtx-video', value)}
        >
          <option value="">Use the host setting</option>
          <option value="on">On</option>
          <option value="off">Off</option>
        </select>
      </Field>
      <div class="numbers">
        <NumberField {draft} key="rtx-video-intensity" label="Strength" hint="0 to 100." placeholder="Host setting" />
      </div>
    {:else}
      <Banner tone="danger" title="RTX Video enhancement is unavailable on this PC">
        {offered.errors.rtx_video ?? 'The NVIDIA graphics card or driver does not offer it.'}
      </Banner>
    {/if}
  </section>
  <p class="note">Empty fields use the host setting. A device can turn either on or off for its own streams.</p>
</div>

<style>
  .rtx {
    display: grid;
    gap: var(--space-5);
  }
  section {
    display: grid;
    gap: var(--space-4);
  }
  h3 {
    margin: 0;
    font-size: var(--text-sm);
  }
  .numbers {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 170px), 1fr));
    gap: var(--space-4) var(--space-5);
  }
  .note {
    font-size: var(--text-xs);
    color: var(--muted);
  }
</style>
