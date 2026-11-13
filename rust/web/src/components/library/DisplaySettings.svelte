<script lang="ts">
  import Field from '../Field.svelte';
  import Toggle from '../Toggle.svelte';
  import ChoiceField, { type Choice } from './ChoiceField.svelte';
  import NumberField from './NumberField.svelte';
  import type { AppDraft, FlagKey } from './draft.svelte';

  let { draft }: { draft: AppDraft } = $props();

  const uid = $props.id();

  const MODES: Choice[] = [
    { value: 'disabled', label: 'Off, stream a monitor' },
    { value: 'per_client', label: 'One for each device' },
    { value: 'shared', label: 'One shared by all devices' },
  ];

  const LAYOUTS: Choice[] = [
    {
      value: 'exclusive',
      label: 'Exclusive',
      hint: 'Turns your monitors off while a device streams this app, so it opens on the stream. They come back on when the stream ends.',
    },
    {
      value: 'extended',
      label: 'Extend — keep PC display primary',
      hint: 'Adds the virtual display beside your monitors and keeps the PC display as the main screen. Games open on the main screen, so on your monitor and not on the stream.',
    },
    {
      value: 'extended_primary',
      label: 'Extend — make virtual display primary',
      hint: 'Your monitors stay on and the virtual display becomes the main screen, with the desktop and taskbar, so games open on the stream.',
    },
    {
      value: 'extended_isolated',
      label: 'Isolate — keep PC display primary',
      hint: 'Keeps the PC display as the main screen, where games open, and places the virtual display far away so the mouse cannot wander onto it.',
    },
    {
      value: 'extended_primary_isolated',
      label: 'Isolate — make virtual display primary',
      hint: 'The virtual display becomes the main screen, so games open on the stream, and is placed far from your other monitors so the mouse cannot wander onto it.',
    },
  ];

  const CAPTURE: Choice[] = [
    { value: 'display', label: 'The display' },
    {
      value: 'window',
      label: 'The app’s window',
      hint: 'Only the app’s window, kept in shape. While it is minimised, not yet open or closed, the stream shows the display and the device says why.',
    },
  ];

  const CLOSED: Choice[] = [
    { value: 'display', label: 'Show the display' },
    { value: 'end', label: 'End the stream' },
  ];

  /** app_window.rs wants_window. */
  const streamsWindow = $derived(
    ['window', 'app', 'true', 'on', 'yes', '1', 'enabled'].includes(draft.text('capture-window').trim().toLowerCase()),
  );

  const ACTIVATION: Choice[] = [
    { value: 'verify_only', label: 'Only check that it’s on' },
    { value: 'ensure_active', label: 'Turn it on' },
    { value: 'ensure_primary', label: 'Turn it on and make it primary' },
    { value: 'ensure_only_display', label: 'Turn it on and the others off' },
    { value: 'disabled', label: 'Leave the monitors alone' },
  ];
</script>

{#snippet flag(key: FlagKey, label: string, hint?: string)}
  <Toggle {label} {hint} bind:checked={() => draft.flag(key), (value) => draft.setFlag(key, value)} />
{/snippet}

<div class="display">
  <div class="toggles">
    {@render flag(
      'virtual-display',
      'Use a virtual display',
      'Streams a virtual display sized for the device, even when the host streams a monitor.',
    )}
    {@render flag('virtual-display-primary', 'Make the virtual display primary', 'The virtual display becomes the main screen beside your monitors, so this app opens on the stream. Used only while Virtual display layout below is on Use the host setting; a device’s or client’s display choice comes first.')}
    {@render flag(
      'use-app-identity',
      'Keep one display identity for this app',
      'Windows remembers the virtual display’s resolution and layout for this app.',
    )}
    {@render flag(
      'per-client-app-identity',
      'Keep a separate identity for each device',
      'Windows then remembers them for each device on its own. Only with the identity above turned on.',
    )}
  </div>

  <div class="form-grid">
    <ChoiceField
      {draft}
      key="virtual-display-mode"
      label="Virtual display mode"
      hint="Whether this app streams a virtual display sized for each device, one shared by all devices, or a monitor. A device’s own setting wins over this."
      options={MODES}
    />
    <ChoiceField
      {draft}
      key="virtual-display-layout"
      label="Virtual display layout"
      hint="What your monitors do while this app streams on a virtual display."
      options={LAYOUTS}
    />
    <Field
      label="Display"
      id="{uid}-output"
      hint="Device ID or name of the monitor to stream. Empty uses the device’s or the host’s choice."
    >
      <input
        id="{uid}-output"
        class="input mono"
        autocomplete="off"
        spellcheck="false"
        placeholder="Use the host setting"
        bind:value={() => draft.text('display-output'), (value) => draft.setText('display-output', value)}
      />
    </Field>
    <ChoiceField
      {draft}
      key="dd-configuration-option"
      label="Monitor setup"
      hint="What happens to the monitor when the stream doesn’t use a virtual display."
      options={ACTIVATION}
    />
    <ChoiceField
      {draft}
      key="capture-window"
      label="What to stream"
      hint="The display, or only this app’s window. A device’s setting for this app wins over this."
      options={CAPTURE}
    />
    {#if streamsWindow}
      <Field
        label="Window’s program"
        id="{uid}-window-exe"
        hint="Only windows of this program, also when a launcher started it. Empty takes any program the app started."
      >
        <input
          id="{uid}-window-exe"
          class="input mono"
          autocomplete="off"
          spellcheck="false"
          placeholder="game.exe"
          bind:value={() => draft.text('capture-window-exe'), (value) => draft.setText('capture-window-exe', value)}
        />
      </Field>
      <Field label="Window title contains" id="{uid}-window-title" hint="Only windows whose title contains this text, in any case. Empty takes any title.">
        <input
          id="{uid}-window-title"
          class="input"
          autocomplete="off"
          placeholder="Any title"
          bind:value={() => draft.text('capture-window-title'), (value) => draft.setText('capture-window-title', value)}
        />
      </Field>
      <ChoiceField
        {draft}
        key="capture-window-closed"
        label="When the window closes"
        hint="Once no other window appeared within 5 seconds."
        options={CLOSED}
      />
    {/if}
    <NumberField
      {draft}
      key="scale-factor"
      label="Render scale"
      hint="Renders the app at this percent of the stream resolution: the streamed display is set to that size. 100 follows the device; another value replaces a scale the device asks for."
      placeholder="100"
      unit="%"
    />
  </div>
</div>

<style>
  .display {
    display: grid;
    gap: var(--space-5);
  }
  .toggles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 280px), 1fr));
    gap: var(--space-4) var(--space-5);
  }
</style>
