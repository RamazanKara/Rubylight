<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../components/Button.svelte';
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

<div class="auth">
  <div class="brand"><span class="mark" aria-hidden="true">R</span>Rubylight</div>
  <form class="card" onsubmit={submit}>
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
</div>

<style>
  .auth {
    min-height: 100vh;
    display: grid;
    place-content: center;
    gap: var(--space-5);
    padding: var(--space-5);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-family: var(--font-display);
    font-weight: 650;
    font-size: 19px;
  }
  .mark {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 7px;
    background: var(--accent);
    color: var(--accent-ink);
    font-weight: 800;
  }
  .card {
    width: min(440px, calc(100vw - 48px));
    display: grid;
    gap: var(--space-4);
    padding: var(--space-6);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
  }
  @media (max-width: 520px) {
    .card {
      padding: var(--space-5);
    }
  }
  h1 {
    font-size: var(--text-xl);
  }
  .remember {
    display: flex;
    gap: 8px;
    align-items: center;
    font-size: var(--text-sm);
  }
  .forgot {
    display: grid;
    gap: var(--space-2);
    font-size: var(--text-sm);
  }
  .forgot summary {
    cursor: pointer;
    color: var(--muted);
  }
  .forgot code {
    display: block;
    overflow-wrap: anywhere;
    user-select: all;
  }
</style>
