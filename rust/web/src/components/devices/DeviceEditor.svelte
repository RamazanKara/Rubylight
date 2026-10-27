<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import Button from '../Button.svelte';
  import Field from '../Field.svelte';
  import Toggle from '../Toggle.svelte';
  import CommandList from './CommandList.svelte';
  import { api, type Client, type DisplayDevice } from '../../lib/api';
  import { failed, notify } from '../../lib/feedback.svelte';
  import {
    changes,
    DISPLAY_OVERRIDE_OPTIONS,
    fullControl,
    permissionSummary,
    PERMISSION_GROUPS,
    savingEndsStream,
    toDraft,
    VIEW_ONLY,
    VIRTUAL_DISPLAY_LAYOUTS,
    VIRTUAL_DISPLAY_MODES,
    validDisplayMode,
    type DisplayOverride,
    type Draft,
  } from './device';
  import { link } from '../../lib/router.svelte';
  import { grantRows, readFolders, withGrant, type FolderAccess, type SharedFolder } from '../../lib/shared-folders';
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
  const granted = () => permissionSummary(draft.perm) === 'Custom' || draft.host_control || draft.allow_sleep || draft.allow_usb;
  let permissionsOpen = $state(untrack(granted));
  $effect(() => {
    if (granted()) permissionsOpen = true;
  });

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
        const read = readFolders(config.shared_folders);
        sharedFolders = read.folders;
        if (read.problem) foldersError = `The Shared folders setting can’t be read, so the PC shares no folder: ${read.problem}.`;
      })
      .catch((error: unknown) => {
        sharedFolders = [];
        foldersError = `Could not read the shared folders: ${reason(error)}`;
      });
  });

  const folderRows = $derived(grantRows(sharedFolders ?? [], draft.shared_folders));

  function setFolderAccess(id: string, access: string) {
    draft.shared_folders = withGrant(draft.shared_folders, id, access === 'read' || access === 'write' ? (access as FolderAccess) : '');
  }

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

  const profileOptions = $derived.by(() => {
    const current = draft.hdr_profile;
    const options = (profiles ?? []).map((filename) => ({ value: filename, label: filename }));
    if (current && !profiles?.includes(current)) {
      options.unshift({ value: current, label: profiles ? `${current} (not found)` : current });
    }
    return options;
  });

  const layoutHint = $derived(
    VIRTUAL_DISPLAY_LAYOUTS.find((layout) => layout.value === draft.virtual_display_layout)?.hint ??
      'How the virtual display sits beside the monitors.',
  );

  const presetHint = $derived(
    presets.find((preset) => preset.id === draft.stream_preset)?.description ??
      'Fills the encoder settings you have not set. Follows the preset in Settings unless this device names its own.',
  );
  /** A stored display override none of its choices shows, as the preset select does. */
  function unknownDisplay(key: DisplayOverride): boolean {
    const value = draft.display[key];
    return value !== '' && !DISPLAY_OVERRIDE_OPTIONS[key].some((option) => option.value === value);
  }
  const presetUnknown = $derived(draft.stream_preset !== '' && !presets.some((preset) => preset.id === draft.stream_preset));

  function setPermission(bit: number, granted: boolean) {
    draft.perm = granted ? draft.perm | bit : draft.perm & ~bit;
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!changed || nameMissing || modeInvalid || saving || disabled) return;
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
    <div class="section-head">
      <h3>Permissions</h3>
      <div class="row">
        <span class="muted presets">Set to</span>
        <Button size="sm" onclick={() => (draft.perm = fullControl(draft.perm))}>Full control</Button>
        <Button size="sm" onclick={() => (draft.perm = VIEW_ONLY)}>View only</Button>
      </div>
    </div>
    <p class="note">
      The first paired device gets full control. Later devices can only list apps and view streams.
      File transfer is granted on its own, never with full control.
    </p>
    <p class="note">Current access: <strong>{permissionSummary(draft.perm)}</strong></p>
    <details bind:open={permissionsOpen}><summary>Custom permissions</summary>
    <div class="groups">
      {#each PERMISSION_GROUPS as group (group.name)}
        <fieldset>
          <legend>{group.name}</legend>
          {#each group.items as item (item.bit)}
            <label class="check">
              <input
                type="checkbox"
                checked={(draft.perm & item.bit) === item.bit}
                onchange={(event) => setPermission(item.bit, event.currentTarget.checked)}
              />
              {item.label}
            </label>
          {/each}
          {#if group.name === 'Other'}
            <!-- Not a permission bit: the host keeps it as its own setting for this device. -->
            <label class="check">
              <input type="checkbox" bind:checked={draft.host_control} />
              Host control: change Radeon settings and close games from this device
            </label>
            {#if sleepUnavailable}
              <p class="note">Sleep PC is unavailable: {sleepUnavailable}.</p>
            {:else}
              <label class="check">
                <input type="checkbox" bind:checked={draft.allow_sleep} />
                Sleep PC: put this PC to sleep from this device while nothing is running (Experimental)
              </label>
            {/if}
            <label class="check">
              <input type="checkbox" bind:checked={draft.allow_usb} />
              USB devices: attach USB devices plugged into this device to this PC during a stream (Experimental)
            </label>
          {/if}
        </fieldset>
      {/each}
    </div>
    </details>
  </section>

  <details class="override" bind:open={foldersOpen}><summary>Shared folders (Experimental)</summary>
  <section>
    {#if sharedFolders === null}
      <p class="note">Loading folders…</p>
    {:else}
      {#if foldersError}<p class="error">{foldersError}</p>{/if}
      {#if folderRows.length === 0}
        <p class="note">
          This PC shares no folders. Add them under Shared folders in <a href="/settings/network" use:link>Settings, Network</a>.
        </p>
      {:else}
        <div class="form-grid">
          {#each folderRows as row (row.id)}
            <Field
              label={row.gone ? `${row.id} (no longer shared)` : row.name}
              id="{uid}-folder-{row.id}"
              hint={row.gone ? 'The PC shares no folder with this id now. Choose No access to remove the grant.' : row.path}
            >
              <select
                id="{uid}-folder-{row.id}"
                class="select"
                value={row.access}
                onchange={(event) => setFolderAccess(row.id, event.currentTarget.value)}
              >
                <option value="">No access</option>
                <option value="read">Read: browse and download</option>
                <option value="write">Read and write: also upload</option>
              </select>
            </Field>
          {/each}
        </div>
      {/if}
      <p class="note">
        Folders are shared only here: Full control and the file transfer permissions don’t include them. Taking a folder
        away, or write access to it, disconnects the device.
      </p>
    {/if}
  </section>
  </details>

  <details class="override" open={modeInvalid}><summary>Display overrides</summary>
  <section>
    <p class="note">“Use the host setting” follows the value in Settings.</p>
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
        hint="Width × height × refresh rate, such as 2560x1600x120. Empty uses the mode the device asks for; the stream keeps the device's frame rate."
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
        hint="Whether this device gets its own virtual display or shares one."
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
      </Field>
      <Field
        label="Rotate the display (Experimental)"
        id="{uid}-rotation"
        hint="Match the device gives this device's virtual display its portrait or landscape shape, and turns a monitor to it, also when the device turns during the stream. A monitor turns back after the stream and needs Windows Graphics Capture. Experimental."
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
          label="Rotation (Experimental)"
          id="{uid}-angle"
          hint="The angle the display turns to, clockwise. A virtual display is made in the turned shape instead. Experimental."
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
        hint="The Windows scale of this device's stream display. Set here, it also scales a monitor, which gets its own scale back after the stream (Experimental)."
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
      hint="Creates a virtual display for this device even when the host streams a monitor."
      bind:checked={draft.always_use_virtual_display}
    />
    <Toggle
      label="Remember the resolution and scale I pick"
      hint={`When you change the resolution or the scale of this device's own virtual display in Windows during a stream, such as 1440p at 200% under a 4K stream, the next streams start at them. The stream keeps the resolution the device asks for, and the refresh rate and VRR follow the stream as usual. A display mode above comes first, and a Virtual display scale set in the host settings comes before the kept scale.${draft.remember_display_mode && draft.remembered_resolution ? ` Kept now: ${draft.remembered_resolution}${draft.remembered_scale ? ` at ${draft.remembered_scale}%` : ''}.` : draft.remember_display_mode && draft.remembered_scale ? ` Kept now: ${draft.remembered_scale}% scale.` : ''}`}
      bind:checked={draft.remember_display_mode}
    />
  </section>
  </details>

  <details class="override" open={presetUnknown}><summary>Stream preset</summary>
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
      hint="Lets this device play while someone keeps using the PC. Its games open on a virtual display away from your monitors, take no focus and stream only their own sound. Controllers work; this device's keyboard, mouse, touch and clipboard are off."
      bind:checked={draft.background_stream}
    />
    <p class="note">
      The PC keeps the real focus; Rubylight tells the game it is active so it keeps its sound and frame rate. Games
      that check focus directly, exclusive fullscreen and Steam Input do not work this way.
    </p>
  </section>
  </details>

  <details class="override"><summary>HDR overrides</summary>
  <section>
    <div class="form-grid">
      <Field
        label="Prefer 10-bit SDR"
        id="{uid}-sdr"
        hint="HDR requests from this device stream as 10-bit SDR and the display stays in SDR. Needs HEVC or AV1."
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
        hint="Applied to the display while this device streams in HDR."
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
      hint="An app can still turn these off for its own launches."
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
    <Button type="submit" variant="primary" busy={saving} disabled={!changed || nameMissing || modeInvalid || disabled}>Save</Button>
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
  .section-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--space-2) var(--space-4);
    flex-wrap: wrap;
  }
  .presets {
    font-size: var(--text-sm);
    margin-right: var(--space-1);
  }
  .note {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
  .error {
    font-size: var(--text-xs);
    color: var(--danger);
    line-height: 1.45;
  }
  .note code {
    color: var(--ink-2);
  }
  .toggle-cell {
    align-self: center;
  }
  .groups {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
    gap: var(--space-3);
  }
  fieldset {
    display: grid;
    gap: 6px;
    align-content: start;
    margin: 0;
    padding: var(--space-2) var(--space-3) var(--space-3);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
    min-width: 0;
  }
  legend {
    padding: 0 var(--space-1);
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
    color: var(--muted);
  }
  .check {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .check input {
    margin: 0;
    accent-color: var(--ink-2);
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
    box-shadow: 0 -1px 0 var(--line);
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
