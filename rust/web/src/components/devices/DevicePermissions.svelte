<script lang="ts">
  import { PERMISSION_GROUPS, type Draft } from './device';

  let {
    draft = $bindable(),
    sleepUnavailable = null,
  }: {
    draft: Draft;
    sleepUnavailable?: string | null;
  } = $props();

  function setPermission(bit: number, granted: boolean) {
    draft.perm = granted ? draft.perm | bit : draft.perm & ~bit;
  }
</script>

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
          {item.label}{#if item.detail}: {item.detail}{/if}
        </label>
      {/each}
      {#if group.name === 'Other'}
        <!-- Not a permission bit: the host keeps it as its own setting for this device. -->
        <label class="check">
          <input type="checkbox" bind:checked={draft.host_control} />
          Host control: change graphics driver settings, close games, install a Rubylight update and manage guests from this device
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={draft.pairing_approve} />
          Approve new devices: pair another device from this device
        </label>
        {#if sleepUnavailable}
          <p class="note">Sleep PC is unavailable: {sleepUnavailable}.</p>
        {:else}
          <label class="check">
            <input type="checkbox" bind:checked={draft.allow_sleep} />
            Sleep PC: put this PC to sleep from this device while nothing is running
          </label>
        {/if}
        <label class="check">
          <input type="checkbox" bind:checked={draft.allow_restart_shutdown} />
          Restart and shut down PC: from this device while nothing is running
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={draft.allow_usb} />
          USB devices: attach USB devices plugged into this device to this PC during a stream, while USB forwarding is on in Settings
        </label>
      {/if}
    </fieldset>
  {/each}
</div>

<style>
  .note {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
  .groups {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
    gap: var(--space-3);
  }
  fieldset {
    display: grid;
    gap: var(--space-1-5);
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
</style>
