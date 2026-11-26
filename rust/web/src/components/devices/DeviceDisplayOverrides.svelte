<script lang="ts">
  import Field from '../Field.svelte';
  import Toggle from '../Toggle.svelte';
  import DisplayTopologyCanvas from './DisplayTopologyCanvas.svelte';
  import type { DisplayDevice } from '../../lib/api';
  import {
    DISPLAY_OVERRIDE_OPTIONS,
    learnedLayoutLabel,
    VIRTUAL_DISPLAY_LAYOUTS,
    VIRTUAL_DISPLAY_MODES,
    type DisplayOverride,
    type Draft,
  } from './device';

  let {
    uid,
    draft = $bindable(),
    displays,
    displaysError,
    hostLayout,
    modeInvalid,
  }: {
    uid: string;
    draft: Draft;
    displays: DisplayDevice[] | null;
    displaysError: string;
    hostLayout: string;
    modeInvalid: boolean;
  } = $props();

  const displayOptions = $derived.by(() => {
    const current = draft.output_name_override;
    const options: { value: string; label: string }[] = [];
    for (const display of displays ?? []) {
      // The host matches the override against the device id or the display name.
      const isCurrent =
        current !== '' &&
        [display.device_id, display.display_name].some((value) => value.toLowerCase() === current.toLowerCase());
      const value = isCurrent ? current : display.device_id || display.display_name;
      if (!value || options.some((option) => option.value === value)) continue;
      const name = display.friendly_name || `Display ${options.length + 1}`;
      options.push({ value, label: display.primary ? `${name}, primary` : name });
    }
    if (current && !options.some((option) => option.value === current)) {
      options.unshift({ value: current, label: displays ? 'Saved display (not connected)' : 'Saved display' });
    }
    return options;
  });

  const layoutHint = $derived(
    VIRTUAL_DISPLAY_LAYOUTS.find((layout) => layout.value === draft.virtual_display_layout)?.hint ??
      'What your monitors do while this device streams on a virtual display.',
  );

  /** The layout a stream learned for this device, as the editor names it, while the host's layout still lets it apply. */
  const learnedLayout = $derived(learnedLayoutLabel(draft, hostLayout));

  /** A stored display override none of its choices shows, as the preset select does. */
  function unknownDisplay(key: DisplayOverride): boolean {
    const value = draft.display[key];
    return value !== '' && !DISPLAY_OVERRIDE_OPTIONS[key].some((option) => option.value === value);
  }
</script>

<p class="note">“Use the host setting” follows the value in Settings.</p>
{#if displays?.length}
  <DisplayTopologyCanvas {displays} target={draft.output_name_override} targetLabel="This device streams here" />
{/if}
<div class="form-grid">
  <Field
    label="Display"
    id="{uid}-display"
    hint="The monitor to stream when this device does not use a virtual display."
    error={displaysError || undefined}
  >
    <select id="{uid}-display" class="select" bind:value={draft.output_name_override}>
      <option value="">Use the host setting</option>
      {#if displays === null}
        <option disabled value="-">Loading displays…</option>
      {/if}
      {#each displayOptions as option (option.value)}
        <option value={option.value}>{option.label}</option>
      {/each}
    </select>
  </Field>
  <Field
    label="Display mode"
    id="{uid}-display-mode"
    hint="The resolution and refresh rate of the display this device streams, as width × height × refresh rate, such as 2560x1600x120. The stream keeps the size and frame rate the device asks for, so a game can render at 1440p on a 4K stream. Leave it empty to use the mode the device asks for."
    error={modeInvalid ? 'Enter width × height × refresh rate, such as 1920x1080x59.94.' : undefined}
  >
    <input
      id="{uid}-display-mode"
      class="input mono"
      autocomplete="off"
      spellcheck="false"
      placeholder="Use the device’s display mode"
      bind:value={draft.display_mode}
    />
  </Field>
  <Field
    label="Virtual display mode"
    id="{uid}-mode"
    hint="Whether this device streams a virtual display of its own, sized for it, one shared with the other devices, or a monitor. Leave it on Use the host setting unless this device needs another choice than your other devices."
  >
    <select id="{uid}-mode" class="select" bind:value={draft.virtual_display_mode}>
      <option value="">Use the host setting</option>
      {#each VIRTUAL_DISPLAY_MODES as mode (mode.value)}
        <option value={mode.value}>{mode.label}</option>
      {/each}
    </select>
  </Field>
  <Field label="Virtual display layout" id="{uid}-layout" hint={layoutHint}>
    <select
      id="{uid}-layout"
      class="select"
      bind:value={draft.virtual_display_layout}
      onchange={() => (draft.virtual_display_layout_set_by_user = true)}
    >
      <option value="">Use the host setting</option>
      {#each VIRTUAL_DISPLAY_LAYOUTS as layout (layout.value)}
        <option value={layout.value}>{layout.label}</option>
      {/each}
    </select>
    {#if learnedLayout}
      <p class="learned">
        Learned from your last stream: {learnedLayout}.
        <button type="button" class="link" onclick={() => (draft.virtual_display_layout = '')}>Clear</button>
        it to follow the host setting again.
      </p>
    {/if}
  </Field>
  <Field
    label="Rotate the display"
    id="{uid}-rotation"
    hint="Match the device gives this device's virtual display its portrait or landscape shape, and turns a monitor to it, also when the device turns during the stream. A monitor turns back after the stream and needs Windows Graphics Capture."
    error={unknownDisplay('dd_rotation_option') ? `“${draft.display.dd_rotation_option}” is not a choice, so the host ignores it. Choose one.` : undefined}
  >
    <select id="{uid}-rotation" class="select" bind:value={draft.display.dd_rotation_option}>
      <option value="">Use the host setting</option>
      {#if unknownDisplay('dd_rotation_option')}
        <option value={draft.display.dd_rotation_option}>{draft.display.dd_rotation_option} (not a choice)</option>
      {/if}
      {#each DISPLAY_OVERRIDE_OPTIONS.dd_rotation_option as option (option.value)}
        <option value={option.value}>{option.label}</option>
      {/each}
    </select>
  </Field>
  {#if draft.display.dd_rotation_option === 'manual'}
    <Field
      label="Rotation"
      id="{uid}-angle"
      hint="The angle the display turns to, clockwise. A virtual display is made in the turned shape instead."
      error={unknownDisplay('dd_manual_rotation') ? `${draft.display.dd_manual_rotation} is not an angle the host takes. Choose one.` : undefined}
    >
      <select id="{uid}-angle" class="select" bind:value={draft.display.dd_manual_rotation}>
        <option value="">Use the host setting</option>
        {#if unknownDisplay('dd_manual_rotation')}
          <option value={draft.display.dd_manual_rotation}>{draft.display.dd_manual_rotation} (not an angle)</option>
        {/if}
        {#each DISPLAY_OVERRIDE_OPTIONS.dd_manual_rotation as option (option.value)}
          <option value={option.value}>{option.label}</option>
        {/each}
      </select>
    </Field>
  {/if}
  <Field
    label="Display scale"
    id="{uid}-scale"
    hint="The Windows scale (text and app size) of this device's stream display; a fixed percentage Windows does not offer at that resolution stops its streams from starting. Set here, it also scales a monitor, which gets its own scale back after the stream. A scale the device asks for itself (Nightjar's PC display scaling) comes first on its virtual display."
    error={unknownDisplay('dd_virtual_display_scale') ? `${draft.display.dd_virtual_display_scale} is not a scale the host takes. Choose one.` : undefined}
  >
    <select id="{uid}-scale" class="select" bind:value={draft.display.dd_virtual_display_scale}>
      <option value="">Use the host setting</option>
      {#if unknownDisplay('dd_virtual_display_scale')}
        <option value={draft.display.dd_virtual_display_scale}>{draft.display.dd_virtual_display_scale} (not a scale)</option>
      {/if}
      {#each DISPLAY_OVERRIDE_OPTIONS.dd_virtual_display_scale as option (option.value)}
        <option value={option.value}>{option.label}</option>
      {/each}
    </select>
  </Field>
</div>
<Toggle
  label="Always use a virtual display"
  hint="Streams a virtual display sized for this device, even when the host streams a monitor."
  bind:checked={draft.always_use_virtual_display}
/>
<Toggle
  label="Remember the resolution and scale I pick"
  hint={`When you change the resolution or the scale of this device's own virtual display in Windows during a stream, such as 1440p at 200% under a 4K stream, the next streams start at them. The stream keeps the resolution the device asks for, and the refresh rate and VRR follow the stream as usual. A display mode above comes first, and a Virtual display scale set in the host settings comes before the kept scale.${draft.remember_display_mode && draft.remembered_resolution ? ` Kept now: ${draft.remembered_resolution}${draft.remembered_scale ? ` at ${draft.remembered_scale}%` : ''}.` : draft.remember_display_mode && draft.remembered_scale ? ` Kept now: ${draft.remembered_scale}% scale.` : ''}`}
  bind:checked={draft.remember_display_mode}
/>

<style>
  .note {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
  .learned {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
  .learned .link {
    border: 0;
    padding: 0;
    background: none;
    color: var(--ink-2);
    font-size: inherit;
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
    min-width: 0;
    min-height: 0;
  }
  .learned .link:hover {
    color: var(--ink);
  }
</style>
