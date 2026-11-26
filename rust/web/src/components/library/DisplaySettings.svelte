<script lang="ts">
  import Field from '../Field.svelte';
  import Toggle from '../Toggle.svelte';
  import ChoiceField, { type Choice } from './ChoiceField.svelte';
  import NumberField from './NumberField.svelte';
  import { ALWAYS, type AppDraft, type FlagKey } from './draft.svelte';

  let { draft }: { draft: AppDraft } = $props();

  const uid = $props.id();

  const MODES: Choice[] = [
    { value: 'disabled', label: 'Off, stream a monitor' },
    { value: 'per_client', label: 'One for each device' },
    { value: 'shared', label: 'One shared by all devices' },
    {
      value: ALWAYS,
      label: 'Always, even where a monitor is set',
      hint: 'A virtual display sized for the device, even on a device or a host set to stream a monitor. Whether devices share one follows their setting, then the host’s.',
    },
  ];
  const MODE_HINT =
    'Whether this app streams a virtual display sized for each device, one shared by all devices, or a monitor. A device’s own setting wins over this, except Always.';
  /** An app saved with Always and a mode of its own beside it, which still decides which display is used. */
  const ALWAYS_WITH: Record<string, Choice> = {
    [`${ALWAYS}:per_client`]: { value: `${ALWAYS}:per_client`, label: 'Always, one for each device' },
    [`${ALWAYS}:shared`]: { value: `${ALWAYS}:shared`, label: 'Always, one shared by all devices' },
    [`${ALWAYS}:disabled`]: { value: `${ALWAYS}:disabled`, label: 'Always, one for each device (saved with Off)' },
  };
  const modeChoice = $derived(draft.virtualDisplay());
  const modeOptions = $derived(ALWAYS_WITH[modeChoice] ? [...MODES, ALWAYS_WITH[modeChoice]] : MODES);
  const modeHint = $derived(MODES.find((option) => option.value === modeChoice)?.hint ?? MODE_HINT);
  /** display_session.rs inherited_layout: the app's own layout comes before its primary switch. */
  const layoutSet = $derived(!['', 'global'].includes(draft.text('virtual-display-layout').trim()));

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
    {
      value: 'window_focus',
      label: 'The app’s window, the display while it’s in the background',
      hint: 'The app’s window while it is in front. After Alt+Tab or a click outside it, the stream shows the display until the app is in front again.',
    },
  ];

  const CLOSED: Choice[] = [
    { value: 'display', label: 'Show the display' },
    { value: 'end', label: 'End the stream' },
  ];

  const FRAME: Choice[] = [
    { value: 'full', label: 'Full window' },
    {
      value: 'client',
      label: 'Only the game’s content',
      hint: 'Crops the title bar and borders. A game that draws its own title bar keeps it.',
    },
  ];

  /** app_window.rs wants_window. */
  const streamsWindow = $derived(
    ['window', 'app', 'true', 'on', 'yes', '1', 'enabled', 'window_focus', 'focus'].includes(
      draft.text('capture-window').trim().toLowerCase(),
    ),
  );

  const ACTIVATION: Choice[] = [
    { value: 'verify_only', label: 'Only check that it’s on' },
    { value: 'ensure_active', label: 'Turn it on' },
    { value: 'ensure_primary', label: 'Turn it on and make it primary' },
    { value: 'ensure_only_display', label: 'Turn it on and the others off' },
    { value: 'disabled', label: 'Leave the monitors alone' },
  ];
</script>

{#snippet flag(key: FlagKey, label: string, hint?: string, disabled = false)}
  <Toggle {label} {hint} {disabled} bind:checked={() => draft.flag(key), (value) => draft.setFlag(key, value)} />
{/snippet}

<div class="display">
  <div class="toggles">
    {@render flag(
      'virtual-display-primary',
      'Make the virtual display primary',
      layoutSet
        ? 'Not used: Virtual display layout below is set, and its choice decides which screen is primary.'
        : 'The virtual display becomes the main screen beside your monitors, so this app opens on the stream. A device’s or client’s display choice comes first.',
      layoutSet,
    )}
    {@render flag(
      'use-app-identity',
      'Keep one display identity for this app',
      'Windows remembers the virtual display’s resolution and layout for this app.',
    )}
    {@render flag(
      'per-client-app-identity',
      'Keep a separate identity for each device',
      draft.flag('use-app-identity')
        ? 'Windows then remembers them for each device on its own.'
        : 'Not used: turn on Keep one display identity for this app first.',
      !draft.flag('use-app-identity'),
    )}
  </div>

  <div class="form-grid">
    <!-- One choice for virtual-display-mode and the virtual-display switch (display_session.rs). -->
    <Field label="Virtual display mode" id="{uid}-mode" hint={modeHint}>
      <select
        id="{uid}-mode"
        class="select"
        bind:value={() => modeChoice, (value) => draft.setVirtualDisplay(value)}
      >
        <option value="">Use the host setting</option>
        {#if modeChoice && !modeOptions.some((option) => option.value === modeChoice)}
          <option value={modeChoice}>Saved custom choice</option>
        {/if}
        {#each modeOptions as option (option.value)}
          <option value={option.value}>{option.label}</option>
        {/each}
      </select>
    </Field>
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
      <ChoiceField
        {draft}
        key="capture-window-frame"
        label="Window’s frame"
        hint="The whole window, or only the game’s content without the title bar and borders. Leave it on Full window unless the game runs windowed and you want only its content."
        options={FRAME}
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
