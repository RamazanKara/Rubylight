<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import Button from '../Button.svelte';
  import Field from '../Field.svelte';
  import Toggle from '../Toggle.svelte';
  import AccessChoice from './AccessChoice.svelte';
  import CommandList from './CommandList.svelte';
  import DeviceCouchOverrides from './DeviceCouchOverrides.svelte';
  import DeviceDisplayOverrides from './DeviceDisplayOverrides.svelte';
  import DeviceFoldersSection from './DeviceFoldersSection.svelte';
  import DevicePermissions from './DevicePermissions.svelte';
  import { accessOf, coopJoinAfterAccess, coopJoinAfterSwitch, withAccess, type Access } from './access';
  import { api, PERM, type Client, type DisplayDevice } from '../../lib/api';
  import { joinsCoop } from '../../lib/coop-view';
  import { failed, notify } from '../../lib/feedback.svelte';
  import {
    changes,
    COUCH_KEYBOARD_CHORDS,
    COUCH_OPEN_WITH_OPTIONS,
    GAMEPAD_OPTIONS,
    LTR_FRAME_OPTIONS,
    validCouchHold,
    validReleaseDelay,
    savingEndsStream,
    toDraft,
    validDisplayMode,
    type Draft,
  } from './device';
  import { readFolders, type SharedFolder } from '../../lib/shared-folders';
  import { session } from '../../lib/session.svelte';
  import { presets } from '../../lib/stream-presets';

  let {
    client,
    sleepUnavailable = null,
    dirty = $bindable(false),
    saving = $bindable(false),
    disabled = false,
    onsaved,
  }: {
    client: Client;
    /** Why this PC offers no Sleep PC; null when it does. */
    sleepUnavailable?: string | null;
    dirty?: boolean;
    saving?: boolean;
    disabled?: boolean;
    onsaved?: () => Promise<void>;
  } = $props();

  const uid = $props.id();

  // `saved` is what the host has; `draft` is what the form shows.
  let saved = $state<Draft>(untrack(() => toDraft(client)));
  let draft = $state<Draft>(untrack(() => toDraft(client)));

  const pending = $derived(changes(saved, draft));
  const changed = $derived(Object.keys(pending).length > 0);
  const nameMissing = $derived(draft.name.trim() === '');
  const modeInvalid = $derived(!validDisplayMode(draft.display_mode));

  $effect(() => {
    dirty = changed;
  });

  // Custom permissions, or a granted Host control, Sleep PC or USB devices, open the section.
  // Nothing closes it again: unticking a box must not snap it shut under the cursor.
  const granted = () =>
    accessOf(draft.perm) === 'custom' || draft.host_control || draft.pairing_approve || draft.allow_sleep || draft.allow_restart_shutdown || draft.allow_usb;
  let permissionsOpen = $state(untrack(granted));
  $effect(() => {
    if (granted()) permissionsOpen = true;
  });
  // Custom stays chosen while the boxes happen to match a preset.
  let customPicked = $state(false);
  const access = $derived.by(() => {
    const preset = accessOf(draft.perm);
    return customPicked || preset === 'none' ? 'custom' : preset;
  });
  function chooseAccess(next: Access | 'custom') {
    customPicked = next === 'custom';
    if (next === 'custom') permissionsOpen = true;
    else {
      draft.perm = withAccess(draft.perm, next);
      // A preset decides co-op too (Play games and Everything join, Watch only does not),
      // except that a device turned off stays off.
      draft.coop_join = coopJoinAfterAccess(draft.coop_join, draft.perm);
    }
  }

  // "Can join co-op games": while it matches Launch apps the device follows it (null), so a
  // later preset change carries it along; a stored value the switch lands back on stays as stored.
  // Joining is a launch of the co-op entry, so without Launch apps the switch stays off.
  const launches = $derived((draft.perm & PERM.LAUNCH_APPS) !== 0);
  const joins = $derived(joinsCoop(draft.perm, draft.coop_join));
  function setJoins(next: boolean) {
    draft.coop_join = coopJoinAfterSwitch(next, saved.coop_join);
  }

  // Follow edits made elsewhere while this form has none of its own.
  $effect(() => {
    const latest = toDraft(client);
    untrack(() => {
      if (!changed && Object.keys(changes(saved, latest)).length > 0) {
        saved = latest;
        draft = toDraft(client);
      }
    });
  });

  let displays = $state<DisplayDevice[] | null>(null);
  let displaysError = $state('');
  let profiles = $state<string[] | null>(null);
  let profilesError = $state('');
  let sharedFolders = $state<SharedFolder[] | null>(null);
  let foldersError = $state('');
  /** The host's virtual display layout; empty until the configuration is read. */
  let hostLayout = $state('');
  // Open while the device holds a grant; choosing No access must not snap it shut.
  let foldersOpen = $state(untrack(() => Object.keys(draft.shared_folders).length > 0));

  const reason = (error: unknown) => (error instanceof Error ? error.message : String(error));

  onMount(() => {
    api.displays
      .devices()
      .then((list) => (displays = list))
      .catch((error: unknown) => {
        displays = [];
        displaysError = `Could not list displays: ${reason(error)}`;
      });
    api.clients
      .hdrProfiles()
      .then((result) => (profiles = result.profiles.map((profile) => profile.filename)))
      .catch((error: unknown) => {
        profiles = [];
        profilesError = `Could not list color profiles: ${reason(error)}`;
      });
    api.config
      .get()
      .then((config) => {
        hostLayout = typeof config.virtual_display_layout === 'string' ? config.virtual_display_layout : '';
        const read = readFolders(config.shared_folders);
        sharedFolders = read.folders;
        if (read.problem) foldersError = `The Shared folders setting can’t be read, so the PC shares no folder: ${read.problem}.`;
      })
      .catch((error: unknown) => {
        sharedFolders = [];
        foldersError = `Could not read the shared folders: ${reason(error)}`;
      });
  });

  const profileOptions = $derived.by(() => {
    const current = draft.hdr_profile;
    const options = (profiles ?? []).map((filename) => ({ value: filename, label: filename }));
    if (current && !profiles?.includes(current)) {
      options.unshift({ value: current, label: profiles ? `${current} (not found)` : current });
    }
    return options;
  });

  const audioSinkOptions = $derived.by(() => {
    const current = draft.audio_sink;
    const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
    const options: { value: string; label: string }[] = [];
    const sinks = session.metadata?.audio_sinks;
    for (const sink of sinks ?? []) {
      const matches =
        current !== '' &&
        [sink.id, sink.name, sink.description, sink.adapter].some((field) => field && same(field, current));
      const choice = matches ? current : sink.id;
      if (!choice || options.some((option) => option.value === choice)) continue;
      const marks: string[] = [];
      if (sink.default) marks.push('default');
      if (sink.virtual_sink && !sink.name.toLowerCase().includes('steam streaming speakers')) {
        marks.push('Steam Streaming Speakers');
      }
      options.push({
        value: choice,
        label: [sink.name || sink.description || 'Unnamed audio device', ...marks].join(' · '),
      });
    }
    if (current && !options.some((option) => option.value === current)) {
      options.unshift({ value: current, label: sinks ? `${current} (not found)` : current });
    }
    return options;
  });

  const presetHint = $derived(
    presets.find((preset) => preset.id === draft.stream_preset)?.description ??
      'Fills in the encoder settings you have not set yourself, for this device’s streams. Leave it on Use the host setting, which follows the preset in Settings, unless this device needs its own.',
  );
  const presetUnknown = $derived(draft.stream_preset !== '' && !presets.some((preset) => preset.id === draft.stream_preset));
  // An invalid display mode or an unknown preset opens its section, as with Custom permissions.
  // Nothing closes it again: typing a mode that turns valid must not snap it shut under the cursor.
  let displayOpen = $state(untrack(() => modeInvalid));
  let presetOpen = $state(untrack(() => presetUnknown));
  const gamepadUnknown = $derived(draft.gamepad !== '' && !GAMEPAD_OPTIONS.some((option) => option.value === draft.gamepad));
  let controllerOpen = $state(untrack(() => gamepadUnknown));
  let soundOpen = $state(untrack(() => Object.values(draft.audio).some((value) => value !== '') || draft.audio_sink !== ''));
  const openWithUnknown = $derived(
    draft.couch.couch_menu_open_with !== '' &&
      !COUCH_OPEN_WITH_OPTIONS.some((option) => option.value === draft.couch.couch_menu_open_with),
  );
  const chordUnknown = $derived(
    draft.couch.couch_menu_keyboard_chord !== '' &&
      !COUCH_KEYBOARD_CHORDS.some((option) => option.value === draft.couch.couch_menu_keyboard_chord),
  );
  const holdInvalid = $derived(!validCouchHold('couch_menu_hold_ms', draft.couch.couch_menu_hold_ms));
  const cornerInvalid = $derived(!validCouchHold('couch_menu_corner_hold_ms', draft.couch.couch_menu_corner_hold_ms));
  const couchInvalid = $derived(openWithUnknown || chordUnknown || holdInvalid || cornerInvalid);
  let couchOpen = $state(untrack(() => Object.values(draft.couch).some((value) => value !== '')));
  $effect(() => {
    if (couchInvalid) couchOpen = true;
  });
  const ltrUnknown = $derived(draft.ltr_frames !== '' && !LTR_FRAME_OPTIONS.some((option) => option.value === draft.ltr_frames));
  let encoderOpen = $state(untrack(() => ltrUnknown));
  const delayInvalid = $derived(!validReleaseDelay(draft.release_delay));
  let touchOpen = $state(untrack(() => delayInvalid));
  $effect(() => {
    if (delayInvalid) touchOpen = true;
  });
  $effect(() => {
    if (modeInvalid) displayOpen = true;
  });
  $effect(() => {
    if (ltrUnknown) encoderOpen = true;
  });
  $effect(() => {
    if (presetUnknown) presetOpen = true;
  });
  $effect(() => {
    if (gamepadUnknown) controllerOpen = true;
  });

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!changed || nameMissing || modeInvalid || delayInvalid || couchInvalid || saving || disabled) return;
    saving = true;
    // Edits made while the request is out stay unsaved.
    const submitted = $state.snapshot(draft);
    try {
      // null clears a per-device setting, which the Client type does not spell out.
      await api.clients.update({ uuid: client.uuid, ...pending } as Partial<Client> & { uuid: string });
      saved = submitted;
      notify(`Saved ${submitted.name.trim()}.`, 'ok');
      await onsaved?.();
    } catch (error) {
      failed('Saving the device failed', error);
    } finally {
      saving = false;
    }
  }

  function discard() {
    customPicked = false;
    draft = $state.snapshot(saved);
  }
</script>

<form class="editor" onsubmit={save} aria-label="Edit {client.name}">
  <section>
    <h3>General</h3>
    <div class="form-grid">
      <Field label="Name" id="{uid}-name" error={nameMissing ? 'Enter a name.' : undefined}>
        <input id="{uid}-name" class="input" autocomplete="off" required bind:value={draft.name} />
      </Field>
      <div class="toggle-cell">
        <Toggle
          label="Allow this device to connect"
          hint="When off, this device cannot connect. Saving with it off disconnects the device."
          bind:checked={draft.enabled}
        />
      </div>
    </div>
  </section>

  <section>
    <h3>Permissions</h3>
    <AccessChoice legend="This device can" custom value={access} onchange={chooseAccess} />
    <Toggle
      label="Can join co-op games"
      hint={launches
        ? 'Take a free slot in a co-op game another device started on this PC. Turning it off ends this device’s streams.'
        : 'Needs Launch apps, which Play games and Everything include.'}
      checked={joins}
      disabled={!launches}
      onchange={setJoins}
    />
    <p class="note">
      The first paired device gets Everything. Later devices get what was chosen when they paired.
      File transfer and Restart and shut down PC are granted on their own, never with Everything.
    </p>
    <details bind:open={permissionsOpen}><summary>Custom permissions</summary>
      <DevicePermissions bind:draft {sleepUnavailable} />
    </details>
  </section>

  <details class="override" bind:open={foldersOpen}><summary>Shared folders</summary>
  <section>
    <DeviceFoldersSection {uid} bind:draft {sharedFolders} {foldersError} />
  </section>
  </details>

  <details class="override" bind:open={displayOpen}><summary>Display overrides</summary>
  <section>
    <DeviceDisplayOverrides {uid} bind:draft {displays} {displaysError} {hostLayout} {modeInvalid} />
  </section>
  </details>

  <details class="override" bind:open={soundOpen}><summary>Sound</summary>
  <section>
    <div class="form-grid">
      <Field
        label="Stream audio"
        id="{uid}-stream-audio"
        hint="Send this PC's sound to this device, in place of Stream audio in Settings. Choose Off when this device is only used as an extra monitor."
      >
        <select id="{uid}-stream-audio" class="select" bind:value={draft.audio.stream_audio}>
          <option value="">Use the host setting</option>
          <option value="on">On</option>
          <option value="off">Off</option>
        </select>
      </Field>
      <Field
        label="PCM and Dolby Digital audio"
        id="{uid}-passthrough"
        hint="Offer uncompressed PCM, Dolby Digital and Dolby Digital Plus to this device, in place of Settings. Other devices keep Opus."
      >
        <select id="{uid}-passthrough" class="select" bind:value={draft.audio.audio_passthrough}>
          <option value="">Use the host setting</option>
          <option value="on">On</option>
          <option value="off">Off</option>
        </select>
      </Field>
      <Field
        label="Audio device"
        id="{uid}-audio-sink"
        hint="The audio output captured for this device when sound stays on this PC or no virtual speakers exist, in place of Audio device in Settings."
        error={session.metadata?.audio_error ? `Could not list audio devices: ${session.metadata.audio_error}` : undefined}
      >
        <select id="{uid}-audio-sink" class="select" bind:value={draft.audio_sink}>
          <option value="">Use the host setting</option>
          {#if session.metadata === null}
            <option disabled value="-">Loading audio devices…</option>
          {/if}
          {#each audioSinkOptions as option (option.value)}
            <option value={option.value}>{option.label}</option>
          {/each}
        </select>
      </Field>
      <Field
        label="Night mode"
        id="{uid}-night"
        hint="Evens out loud and quiet passages for this device, in place of Night mode in Settings. Leave it on Use the host setting unless this device needs its own."
      >
        <select id="{uid}-night" class="select" bind:value={draft.audio.audio_night_mode}>
          <option value="">Use the host setting</option>
          <option value="on">On</option>
          <option value="off">Off</option>
        </select>
      </Field>
      <Field
        label="Headphone surround"
        id="{uid}-surround"
        hint="For a device that plays stereo, renders the PC's surround channels for headphones so sound keeps its direction, in place of Headphone surround in Settings. Leave it off for speakers."
      >
        <select id="{uid}-surround" class="select" bind:value={draft.audio.audio_headphone_surround}>
          <option value="">Use the host setting</option>
          <option value="on">On</option>
          <option value="off">Off</option>
        </select>
      </Field>
    </div>
  </section>
  </details>

  <details class="override" bind:open={couchOpen}><summary>Couch menu</summary>
  <section>
    <DeviceCouchOverrides {uid} bind:draft {openWithUnknown} {chordUnknown} {holdInvalid} {cornerInvalid} />
  </section>
  </details>

  <details class="override" bind:open={controllerOpen}><summary>Controller</summary>
  <section>
    <div class="form-grid">
      <Field
        label="Controller type"
        id="{uid}-gamepad"
        hint="The controller games see for this device's controllers, in place of the Controller type in Settings. Leave it on Use the host setting unless this device needs its own: Automatic goes by the controller the device reports, and Moonlight on a Nintendo Switch reports none, so choose Switch Pro for it."
        error={gamepadUnknown ? `“${draft.gamepad}” is not a controller type, so the host ignores it. Choose one.` : undefined}
      >
        <select id="{uid}-gamepad" class="select" bind:value={draft.gamepad}>
          <option value="">Use the host setting</option>
          {#if gamepadUnknown}
            <option value={draft.gamepad}>{draft.gamepad} (not a choice)</option>
          {/if}
          {#each GAMEPAD_OPTIONS as option (option.value)}
            <option value={option.value}>{option.label}</option>
          {/each}
        </select>
      </Field>
    </div>
  </section>
  </details>

  <details class="override" bind:open={encoderOpen}><summary>Encoder overrides</summary>
  <section>
    <div class="form-grid">
      <Field
        label="Long-term reference frames"
        id="{uid}-ltr"
        hint="AMD AMF: frames the encoder keeps so this device recovers from lost packets with a small frame instead of a keyframe (a full picture the stream restarts from). Leave it on Use the host setting; choose Off only for a device whose decoder freezes on them, so its picture stops."
        error={ltrUnknown ? `“${draft.ltr_frames}” is not a choice. Choose one.` : undefined}
      >
        <select id="{uid}-ltr" class="select" bind:value={draft.ltr_frames}>
          <option value="">Use the host setting</option>
          {#if ltrUnknown}
            <option value={draft.ltr_frames}>{draft.ltr_frames} (not a choice)</option>
          {/if}
          {#each LTR_FRAME_OPTIONS as option (option.value)}
            <option value={option.value}>{option.label}</option>
          {/each}
        </select>
      </Field>
    </div>
  </section>
  </details>

  <details class="override" bind:open={touchOpen}><summary>Touch and pen</summary>
  <section>
    <div class="form-grid">
      <Field
        label="Touch and pen click delay"
        id="{uid}-release-delay"
        hint="How long a tap on this device's touch screen or pen holds its left-button release back, in milliseconds: 0 to 1000. Raise it if a game misses taps or reads them as a drag; leave it empty to use the host's 10."
        error={delayInvalid ? 'Enter a whole number from 0 to 1000, or leave it empty.' : undefined}
      >
        <input
          id="{uid}-release-delay"
          class="input num"
          type="text"
          inputmode="numeric"
          placeholder="10"
          aria-invalid={delayInvalid ? 'true' : undefined}
          bind:value={draft.release_delay}
        />
      </Field>
    </div>
  </section>
  </details>

  <details class="override" bind:open={presetOpen}><summary>Stream preset</summary>
  <section>
    <div class="form-grid">
      <Field
        label="Stream preset"
        id="{uid}-preset"
        hint={presetHint}
        error={presetUnknown ? `“${draft.stream_preset}” is not a stream preset, so the host ignores it. Choose one.` : undefined}
      >
        <select id="{uid}-preset" class="select" bind:value={draft.stream_preset}>
          <option value="">Use the host setting</option>
          {#if presetUnknown}
            <option value={draft.stream_preset}>{draft.stream_preset} (not a preset)</option>
          {/if}
          {#each presets as preset (preset.id)}
            <option value={preset.id}>{preset.label}</option>
          {/each}
        </select>
      </Field>
    </div>
    <p class="note">
      Settings you set for this device or for an app still win over the preset. An app can name its own preset, which
      comes before this one.
    </p>
  </section>
  </details>

  <details class="override"><summary>Play in the background</summary>
  <section>
    <Toggle
      label="Stream in the background"
      hint="Lets this device play while someone keeps using the PC. Nucleus Co-op runs its games in their own window, away from your monitors, and only their own sound streams. Controllers work; this device's keyboard, mouse, touch and clipboard are off."
      bind:checked={draft.background_stream}
    />
    <p class="note">
      Nucleus Co-op tells the game it has the focus, so the PC keeps its own. A game Nucleus Co-op cannot run this
      way, such as one with anti-cheat, is refused with the reason and never streams your monitors instead. Exclusive
      fullscreen and Steam Input do not work this way.
    </p>
  </section>
  </details>

  <details class="override"><summary>HDR overrides</summary>
  <section>
    <div class="form-grid">
      <Field
        label="Prefer 10-bit SDR"
        id="{uid}-sdr"
        hint="When this device asks for HDR, it gets 10-bit SDR instead and the display stays in SDR: less banding, but no HDR picture. Needs HEVC or AV1."
      >
        <select id="{uid}-sdr" class="select" bind:value={draft.prefer_10bit_sdr}>
          <option value="">Use the host setting</option>
          <option value="on">On</option>
          <option value="off">Off</option>
        </select>
      </Field>
      <Field
        label="HDR color profile"
        id="{uid}-profile"
        hint="A Windows color profile (ICC) applied to the display while this device streams in HDR. Choose one made for this device’s screen; leave it on None otherwise."
        error={profilesError || undefined}
      >
        <select id="{uid}-profile" class="select" bind:value={draft.hdr_profile}>
          <option value="">None</option>
          {#if profiles === null}
            <option disabled value="-">Loading profiles…</option>
          {/if}
          {#each profileOptions as option (option.value)}
            <option value={option.value}>{option.label}</option>
          {/each}
        </select>
      </Field>
    </div>
  </section>
  </details>

  <details class="override"><summary>Commands</summary>
  <section>
    <Toggle
      label="Run connect and disconnect commands"
      hint="Turn it off to keep this device’s commands below without running them. An app can still turn these off for its own launches."
      bind:checked={draft.allow_client_commands}
    />
    <div class="command-lists">
      <CommandList label="Connect" hint="Run on this PC when the device starts streaming." bind:rows={draft.do} />
      <CommandList label="Disconnect" hint="Run on this PC when its stream ends." bind:rows={draft.undo} />
    </div>
    <p class="note">
      Commands can use <code>$(SUNSHINE_CLIENT_NAME)</code>, <code>$(SUNSHINE_CLIENT_WIDTH)</code>,
      <code>$(SUNSHINE_CLIENT_HEIGHT)</code>, <code>$(SUNSHINE_CLIENT_FPS)</code> and <code>$(SUNSHINE_CLIENT_HDR)</code>.
    </p>
  </section>
  </details>

  <footer>
    <Button type="submit" variant="primary" busy={saving} disabled={!changed || nameMissing || modeInvalid || delayInvalid || couchInvalid || disabled}>Save</Button>
    <Button disabled={!changed || saving} onclick={discard}>Discard</Button>
    <span class="status muted">
      {#if savingEndsStream(client.connected, saved, draft)}
        Saving ends this device's current stream.
      {:else if changed}
        Unsaved changes
      {/if}
    </span>
  </footer>
</form>

<style>
  .override > summary { padding: var(--space-4) var(--space-5); font-weight: var(--weight-semibold); border-bottom: 1px solid var(--line); }
  .override[open] > summary { border-bottom: 0; }
  .editor {
    display: grid;
    border-top: 1px solid var(--line);
    background: var(--sunken);
  }
  section {
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4) var(--space-5);
    border-bottom: 1px solid var(--line);
    min-width: 0;
  }
  h3 {
    font-size: var(--text-sm);
  }
  .note {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
  .note code {
    color: var(--ink-2);
  }
  .toggle-cell {
    align-self: center;
  }
  .command-lists {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--space-4) var(--space-5);
  }
  footer {
    position: sticky;
    bottom: 0;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
    padding: var(--space-3) var(--space-5);
    background: var(--panel);
    /* Sits on the last section's rule; stays visible when the footer sticks. */
    box-shadow: var(--rule-above);
  }
  .status {
    font-size: var(--text-xs);
    margin-left: var(--space-2);
  }
  @media (max-width: 900px) {
    .command-lists {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  @media (max-width: 520px) {
    section,
    footer {
      padding-left: var(--space-4);
      padding-right: var(--space-4);
    }
  }
</style>
