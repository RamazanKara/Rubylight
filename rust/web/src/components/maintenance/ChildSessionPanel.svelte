<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Badge from '../Badge.svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import Field from '../Field.svelte';
  import LoadState from '../LoadState.svelte';
  import Panel from '../Panel.svelte';
  import { api, type ChildSessionStatus } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { pending, phases, statusLoader, step } from './child-session';
  import { reason } from './util';

  type Action = 'setup' | 'restore' | 'start' | 'stop' | 'show' | 'end' | 'forget';

  /** Set once the status loads: whether there is anything to do here (a load error counts). */
  let { shown = $bindable(false) }: { shown?: boolean } = $props();

  let status = $state<ChildSessionStatus | null>(null);
  let loadError = $state('');
  let running = $state<Action | null>(null);
  /** Typed here only for one start; cleared right after it is sent. */
  let password = $state('');
  let timer: ReturnType<typeof setInterval> | undefined;

  const viewer = $derived(status?.viewer?.status ?? null);
  const current = $derived(status ? step(status) : null);

  $effect(() => {
    shown = status ? pending(status) : Boolean(loadError);
  });

  const loader = statusLoader(
    () => api.childSession.status(),
    (value) => {
      status = value;
      loadError = '';
    },
    (error) => {
      loadError = reason(error);
    },
  );
  const load = loader.load;

  async function run(action: Action, task: () => Promise<unknown>, done: string, failure: string) {
    running = action;
    try {
      await task();
      notify(done, 'ok');
    } catch (error) {
      failed(failure, error);
    } finally {
      running = null;
    }
    await load();
  }

  async function setup() {
    const ok = await confirm({
      title: 'Set up child sessions?',
      message:
        'Rubylight turns on Remote Desktop for this PC only (the firewall keeps it closed to the network) and Windows child sessions, and records what was there before. Restore puts it all back, and so does uninstalling Rubylight.',
      confirm: 'Set up',
    });
    if (ok) await run('setup', api.childSession.setup, 'Child sessions are set up.', 'Setting up child sessions failed');
  }

  async function restore() {
    const ok = await confirm({
      title: 'Restore the Windows settings?',
      message: 'This detaches the child session and puts Remote Desktop, child sessions and the firewall rules back as they were before setup.',
      confirm: 'Restore',
      danger: true,
    });
    if (ok) await run('restore', api.childSession.restore, 'Windows settings restored.', 'Restoring the Windows settings failed');
  }

  async function start(event: SubmitEvent) {
    event.preventDefault();
    const typed = password;
    password = '';
    await run('start', () => api.childSession.start(typed || undefined), 'Starting the child session.', 'Starting the child session failed');
  }

  async function end() {
    const attached = viewer;
    if (!attached?.child) return;
    const ok = await confirm({
      title: 'Sign out the child session?',
      message: 'Every app open on the child session’s desktop closes without saving.',
      confirm: 'Sign out',
      danger: true,
    });
    if (ok)
      await run('end', () => api.childSession.end(attached.child, attached.generation), 'Signing out the child session.', 'Signing out failed');
  }

  onMount(() => {
    load();
    timer = setInterval(() => {
      if (status?.viewer && !running) loader.tick();
    }, 3000);
  });
  onDestroy(() => clearInterval(timer));
</script>

<Panel
  title="Background child session"
  level={3}
  description="Streams a second desktop of your own account while you keep using this one."
>
  {#if status}
    <div class="stack">
      <dl class="facts">
        <dt>Mode</dt>
        <dd>{status.label}</dd>
        <dt>Why</dt>
        <dd class="muted">{status.reason}</dd>
        {#if viewer}
          <dt>Child session</dt>
          <dd><Badge tone={phases[viewer.phase].tone}>{phases[viewer.phase].label}</Badge> <span class="muted">{viewer.message}</span></dd>
        {/if}
        {#if status.viewer?.host_child && status.host_port}
          <dt>Pair with</dt>
          <dd>The background host on port {status.host_port}</dd>
        {/if}
      </dl>
      {#if status.problem}
        <Banner tone="danger" title="Background streaming stopped">{status.problem}</Banner>
      {/if}
      {#if viewer?.notice}
        <Banner tone="warn">{viewer.notice}</Banner>
      {/if}
      {#if status.missing_error}
        <Banner tone="warn" title="The Windows settings could not be read">{status.missing_error}</Banner>
      {/if}
      {#if status.display_error}
        <Banner tone="warn" title="The child session’s display setting is not valid">{status.display_error}</Banner>
      {/if}
      {#if current === 'off'}
        <p class="muted">Child sessions no longer run: Nucleus Co-op runs background streams.</p>
      {:else if current === 'refused'}
        <Banner tone="danger" title={status.label}>{status.reason}</Banner>
      {:else if current === 'inside'}
        <p class="muted">This is the child session’s own host. Manage the child session from the host on the PC’s screen.</p>
      {:else if current === 'unavailable'}
        <Banner tone="warn" title="Child sessions are unavailable in this host">{status.unavailable}</Banner>
      {:else if current === 'setup' && status.missing && status.missing.length > 0}
        <div class="stack">
          <p>Setup still has to change:</p>
          <ul>
            {#each status.missing as item (item)}<li>{item}</li>{/each}
          </ul>
          <div class="row"><Button busy={running === 'setup'} onclick={setup}>Set up</Button></div>
        </div>
      {/if}
      {#if current === 'ready'}
        <form class="stack" onsubmit={start}>
          <Field
            label="Windows password"
            id="child-session-password"
            hint={viewer?.password_saved
              ? 'Saved, encrypted for your Windows account. Leave it empty to use the saved one.'
              : 'Needed once to sign in the child session; it is saved encrypted for your Windows account.'}
          >
            <input id="child-session-password" class="input" type="password" autocomplete="current-password" bind:value={password} />
          </Field>
          <div class="row">
            <Button type="submit" variant="primary" busy={running === 'start'}>Start</Button>
            {#if viewer}
              <Button busy={running === 'show'} onclick={() => run('show', api.childSession.show, 'Showing the child session.', 'Showing the child session failed')}>Show desktop</Button>
              <Button busy={running === 'stop'} onclick={() => run('stop', api.childSession.stop, 'Child session detached.', 'Detaching failed')}>Detach</Button>
              {#if viewer.child}
                <Button variant="danger" busy={running === 'end'} onclick={end}>Sign out</Button>
              {/if}
              {#if viewer.password_saved}
                <Button variant="ghost" busy={running === 'forget'} onclick={() => run('forget', api.childSession.forgetPassword, 'Saved password forgotten.', 'Forgetting the password failed')}>Forget password</Button>
              {/if}
            {/if}
          </div>
        </form>
      {/if}
      {#if status.set_up && current !== 'unavailable'}
        <div class="row">
          <Button variant="danger" busy={running === 'restore'} onclick={restore}>Restore Windows settings</Button>
        </div>
      {/if}
      <ul class="muted hint">
        {#each status.limits as limit (limit)}<li>{limit}</li>{/each}
      </ul>
    </div>
  {:else}
    <LoadState error={loadError} what="the child session status" onretry={load} />
  {/if}
</Panel>

<style>
  .hint {
    font-size: var(--text-xs);
  }
</style>
