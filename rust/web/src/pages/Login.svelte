<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../components/Button.svelte';
  import AuthLayout from '../components/AuthLayout.svelte';
  import Field from '../components/Field.svelte';
  import { api } from '../lib/api';
  import { navigate, query } from '../lib/router.svelte';
  import { checkSession } from '../lib/session.svelte';

  let remember = $state(true);
  let busy = $state(false);
  let error = $state('');
  /** Set on this PC: the program that sets a new sign-in. */
  let program = $state('');

  onMount(async () => {
    try {
      program = (await api.auth.status()).creds_program ?? '';
    } catch {
      // The sign-in form works without the reset help.
    }
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    // Read the fields themselves: a browser can fill saved values in
    // without the input events a binding relies on.
    const form = new FormData(event.currentTarget as HTMLFormElement);
    busy = true;
    error = '';
    try {
      await api.auth.login(String(form.get('username') ?? ''), String(form.get('password') ?? ''), remember);
      const next = query('next');
      navigate(next && next.startsWith('/') && !next.startsWith('//') ? next : '/', { replace: true });
      await checkSession();
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      busy = false;
    }
  }
</script>

<AuthLayout>
  <form onsubmit={submit}>
    <h1>Sign in</h1>
    <p class="muted">
      Use this host's username and password. A profile imported from Apollo, Vibepollo or Sunshine keeps that host's sign-in.
    </p>
    <Field label="Username" id="username">
      <input id="username" name="username" class="input" autocomplete="username" required />
    </Field>
    <Field label="Password" id="password">
      <input id="password" name="password" class="input" type="password" autocomplete="current-password" required />
    </Field>
    <label class="remember"><input type="checkbox" bind:checked={remember} /> Keep me signed in on this browser</label>
    {#if error}<p class="notice danger" role="alert">{error}</p>{/if}
    <Button type="submit" variant="primary" {busy}>Sign in</Button>
    {#if program}
      <details class="forgot">
        <summary>Forgot your sign-in?</summary>
        <p>
          On this PC, open PowerShell as administrator and run this with a new username and a password of at least 8
          characters. Then sign in here with them.
        </p>
        <code>&amp; "{program}" --creds NAME PASSWORD</code>
      </details>
    {/if}
  </form>
</AuthLayout>

<style>
  .remember {
    display: flex;
    min-height: var(--control-height);
    gap: var(--space-3);
    align-items: center;
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .remember input {
    width: 18px;
    height: 18px;
    margin: 0;
  }
  .forgot {
    font-size: var(--text-sm);
    color: var(--ink-2);
  }
  .forgot summary {
    width: fit-content;
    color: var(--muted);
  }
  .forgot p {
    margin-bottom: var(--space-2);
  }
  .forgot code {
    display: block;
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--sunken);
    color: var(--ink);
    overflow-wrap: anywhere;
    user-select: all;
  }
</style>
