<script lang="ts">
  // Where Nucleus Co-op is: a link to the release's page, which the user downloads and unpacks
  // themselves (Rubylight downloads nothing), and the folder they chose, which the host checks as it's
  // kept. A folder the host refuses stays unsaved and says why.
  import { untrack } from 'svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import PathPicker from '../PathPicker.svelte';
  import { api, type CoopStatus } from '../../lib/api';
  import { nucleusRelease, sentence, UNPACK_WHERE } from '../../lib/coop-view';
  import { notify } from '../../lib/feedback.svelte';
  import { reason } from '../maintenance/util';

  let {
    status,
    onchosen,
  }: {
    status: CoopStatus;
    /** After the host kept a folder: the panel loads its status again. */
    onchosen: () => void;
  } = $props();

  const uid = $props.id();
  const release = $derived(nucleusRelease(status));
  let path = $state(untrack(() => status.install.chosen ?? ''));
  let busy = $state(false);
  /** Why the host refused the folder last tried; it was not kept. */
  let refusal = $state('');

  async function choose() {
    busy = true;
    refusal = '';
    try {
      const install = await api.coop.folder(path.trim());
      notify(`Nucleus Co-op ${install.version ?? release.version} found in ${install.path ?? path.trim()}.`, 'ok');
      onchosen();
    } catch (error) {
      refusal = sentence(reason(error));
    } finally {
      busy = false;
    }
  }
</script>

<div class="folder">
  <p class="note">
    <a class="release" href={release.page} target="_blank" rel="noreferrer">Download Nucleus Co-op {release.version}<Icon name="external" size={14} /></a>
    <span class="muted">{UNPACK_WHERE}</span>
  </p>
  <div class="choose">
    <PathPicker kind="directory" value={path} onpick={(picked) => (path = picked)}>
      <input
        id="{uid}-folder"
        class="input mono"
        aria-label="Nucleus Co-op’s folder"
        autocomplete="off"
        spellcheck="false"
        placeholder="C:\Program Files\Nucleus Co-op"
        bind:value={path}
      />
    </PathPicker>
    <Button variant="primary" {busy} disabled={!path.trim()} onclick={choose}>Use this folder</Button>
  </div>
  {#if refusal}
    <Banner tone="danger" title="This folder can’t run Nucleus Co-op" live>{refusal}</Banner>
  {/if}
</div>

<style>
  .folder {
    display: grid;
    gap: var(--space-3);
  }
  .note {
    margin: 0;
    font-size: var(--text-sm);
    line-height: var(--leading-snug);
  }
  .release {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    margin-right: var(--space-2);
  }
  .choose {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: start;
    gap: var(--space-2) var(--space-3);
  }
</style>
