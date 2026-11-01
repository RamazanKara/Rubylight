<script lang="ts">
  // Where Nucleus Co-op is: a link to the release's page, which the user downloads and unpacks
  // themselves (Rubylight downloads nothing), and the folder they chose, which the host checks as it's
  // kept. A folder the host refuses stays unsaved and says why; so does a change or a forget while a co-op
  // session runs. A kept folder that isn't the pinned release runs the earlier install instead (`notice`).
  import { untrack } from 'svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import PathPicker from '../PathPicker.svelte';
  import { api, ApiError, type CoopStatus } from '../../lib/api';
  import { EARLIER_COPY_GAMES, earlierCopyRuns, nucleusRelease, refusalTitle, sentence, UNPACK_WHERE } from '../../lib/coop-view';
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
  /** 'choose' or 'forget' while its request is out. */
  let busy = $state('');
  /** Why the host refused the folder last tried, or forgetting it; nothing was kept. */
  let refusal = $state<{ title: string; text: string } | null>(null);
  /** A folder was just kept in place of the earlier install, whose games stay in it. */
  let replaced = $state(false);

  async function choose() {
    busy = 'choose';
    refusal = null;
    const earlier = earlierCopyRuns(status.install);
    try {
      const install = await api.coop.folder(path.trim());
      notify(`Nucleus Co-op ${install.version ?? release.version} found in ${install.path ?? path.trim()}.`, 'ok');
      replaced = earlier;
      onchosen();
    } catch (error) {
      refusal = { title: refusalTitle(error instanceof ApiError ? error.status : undefined, false), text: sentence(reason(error)) };
    } finally {
      busy = '';
    }
  }

  async function forget() {
    busy = 'forget';
    refusal = null;
    try {
      await api.coop.folder('');
      notify('Nucleus Co-op’s folder forgotten.', 'ok');
      path = '';
      replaced = false;
      onchosen();
    } catch (error) {
      refusal = { title: refusalTitle(error instanceof ApiError ? error.status : undefined, true), text: sentence(reason(error)) };
    } finally {
      busy = '';
    }
  }
</script>

<div class="folder">
  {#if status.install.notice}
    <Banner tone="warn" title="Rubylight runs the earlier copy">{sentence(status.install.notice)}</Banner>
  {/if}
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
    <Button variant="primary" busy={busy === 'choose'} disabled={!path.trim() || busy !== ''} onclick={choose}>Use this folder</Button>
  </div>
  {#if status.install.chosen || replaced || earlierCopyRuns(status.install)}
    <div class="after">
      {#if replaced || earlierCopyRuns(status.install)}<span class="muted">{EARLIER_COPY_GAMES}</span>{/if}
      {#if status.install.chosen}
        <Button variant="ghost" size="sm" busy={busy === 'forget'} disabled={busy !== ''} onclick={forget}>Forget this folder</Button>
      {/if}
    </div>
  {/if}
  {#if refusal}
    <Banner tone="danger" title={refusal.title} live>{refusal.text}</Banner>
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
  .after {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2) var(--space-3);
    font-size: var(--text-sm);
    line-height: var(--leading-snug);
  }
  .choose {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: start;
    gap: var(--space-2) var(--space-3);
  }
  /* A phone: the path gets the whole row, the button goes below it. */
  @media (max-width: 520px) {
    .choose {
      grid-template-columns: 1fr;
    }
    .choose > :global(:last-child) {
      justify-self: start;
    }
  }
</style>
