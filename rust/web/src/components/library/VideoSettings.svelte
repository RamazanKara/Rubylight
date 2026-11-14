<script lang="ts">
  import Field from '../Field.svelte';
  import Toggle from '../Toggle.svelte';
  import ChoiceField, { type Choice } from './ChoiceField.svelte';
  import type { AppDraft } from './draft.svelte';

  let { draft }: { draft: AppDraft } = $props();

  const uid = $props.id();

  const GENERATION: Choice[] = [
    { value: 'game-provided', label: 'Provided by the game', hint: 'The game adds generated frames, e.g. FSR frame generation.' },
    {
      value: 'lossless-scaling',
      label: 'Lossless Scaling',
      hint: 'Lossless Scaling adds generated frames; the game is limited to half the target. See Lossless Scaling below.',
    },
  ];

  const GAMEPADS: Choice[] = [
    { value: 'auto', label: 'Automatic' },
    { value: 'vhf_xbox_one', label: 'Xbox One' },
    { value: 'vhf_xbox', label: 'Xbox Series' },
    { value: 'vhf_ds4', label: 'DualShock 4' },
    { value: 'vhf_ds5', label: 'DualSense' },
    { value: 'usbip_ds5', label: 'DualSense with waveform haptics (USB/IP)', hint: 'Requires usbip-win2 on the host and a waveform-capable client. Start the game after connecting the controller.' },
    { value: 'usbip_switch2', label: 'Switch 2 Pro (USB/IP)', hint: 'Requires usbip-win2 on the host. Start the game after connecting the controller.' },
    { value: 'vhf_switch', label: 'Switch Pro' },
    { value: 'vhf_steam', label: 'Steam Controller', hint: 'Requires the current virtual gamepad driver; run Rubylight setup again if the stream says the driver lacks it.' },
    { value: 'vigem_x360', label: 'Xbox 360 (ViGEmBus)', hint: 'Requires ViGEmBus on the host, for tools such as reWASD that may not see the other types.' },
    { value: 'vigem_ds4', label: 'DualShock 4 (ViGEmBus)', hint: 'Requires ViGEmBus on the host, for tools such as reWASD that may not see the other types. Keeps the touchpad, motion and lightbar.' },
  ];
</script>

<div class="video">
  <div class="form-grid">
    <Field
      label="Prefer 10-bit SDR"
      id="{uid}-sdr"
      hint="When on, a device that asks for HDR gets 10-bit SDR instead and the display stays in SDR: less banding, but no HDR picture. Needs HEVC or AV1."
    >
      <select
        id="{uid}-sdr"
        class="select"
        bind:value={() => draft.tristate('prefer-10bit-sdr'), (value) => draft.setTristate('prefer-10bit-sdr', value)}
      >
        <option value="">Use the host setting</option>
        <option value="on">On</option>
        <option value="off">Off</option>
      </select>
    </Field>
    <ChoiceField
      {draft}
      key="frame-generation-mode"
      label="Frame generation"
      hint="Tells the host that this app’s frames are generated, so its frame limiter keeps the stream in pace. Leave it on None unless the game or Lossless Scaling generates frames."
      empty="None"
      options={GENERATION}
    />
    <ChoiceField
      {draft}
      key="gamepad"
      label="Controller type"
      hint="Automatic matches each device’s controller: DualSense for PlayStation or for motion and touchpad input, Switch Pro for Nintendo, Xbox Series otherwise. A choice here uses that controller for this app."
      options={GAMEPADS}
    />
  </div>

  <div class="toggles">
    <Toggle
      label="Legacy frame generation capture fix"
      hint="An older way to mark the app as generating frames, so the host’s frame limiter paces it on a streamed monitor. Only used when Frame generation is set to None; choose Frame generation above for new apps."
      bind:checked={() => draft.flag('gen1-framegen-fix'), (value) => draft.setFlag('gen1-framegen-fix', value)}
    />
    <Toggle
      label="Frame generation limiter fix"
      hint="The host does not read this switch; its saved value is only kept. Leave it off."
      bind:checked={() => draft.flag('frame-gen-limiter-fix'), (value) => draft.setFlag('frame-gen-limiter-fix', value)}
    />
  </div>
</div>

<style>
  .video {
    display: grid;
    gap: var(--space-5);
  }
  .toggles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 280px), 1fr));
    gap: var(--space-4) var(--space-5);
  }
</style>
