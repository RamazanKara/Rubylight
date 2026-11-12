<script lang="ts">
  import Button from '../components/Button.svelte';
  import AuthLayout from '../components/AuthLayout.svelte';
  import Field from '../components/Field.svelte';
  import { api } from '../lib/api';
  import { navigate } from '../lib/router.svelte';
  import { checkSession } from '../lib/session.svelte';

  let username = $state('');
  let password = $state('');
  let confirmation = $state('');
  let busy = $state(false);
  let error = $state('');
  /** Create account was pressed: missing or short values show their message from then on. */
  let tried = $state(false);

  // The button stays a ready primary button; what is missing shows on the field when it is pressed.
  const problems = $derived({
    username: username.trim() ? '' : 'Enter a username.',
    password: password.length >= 8 ? '' : 'Use at least 8 characters.',
    confirmation: confirmation === password ? '' : 'The passwords do not match.',
  });
  const usernameError = $derived(tried ? problems.username : '');
  const passwordError = $derived(tried ? problems.password : '');
  // A different confirmation shows as soon as one is typed.
  const confirmationError = $derived(tried || confirmation.length > 0 ? problems.confirmation : '');

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    tried = true;
    const first = problems.username ? 'new-username' : problems.password ? 'new-password' : problems.confirmation ? 'confirm-password' : '';
    if (first) {
      document.getElementById(first)?.focus();
      return;
    }
    busy = true;
    error = '';
    try {
      await api.auth.setPassword({ newUsername: username, newPassword: password, confirmNewPassword: confirmation });
      await api.auth.login(username, password, true);
      navigate('/', { replace: true });
      await checkSession();
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      busy = false;
    }
  }
</script>

<AuthLayout>
  <form onsubmit={submit} novalidate>
    <h1>Set up Rubylight</h1>
    <p class="muted">The username and password that sign in to this console, from this PC or any device on your network.</p>
    <Field label="Username" id="new-username" error={usernameError || undefined}>
      <input id="new-username" class="input" autocomplete="username" required aria-invalid={usernameError ? true : undefined} bind:value={username} />
    </Field>
    <Field label="Password" id="new-password" hint="At least 8 characters." error={passwordError || undefined}>
      <input id="new-password" class="input" type="password" autocomplete="new-password" minlength="8" required aria-invalid={passwordError ? true : undefined} bind:value={password} />
    </Field>
    <Field label="Confirm password" id="confirm-password" error={confirmationError || undefined}>
      <input id="confirm-password" class="input" type="password" autocomplete="new-password" required aria-invalid={confirmationError ? true : undefined} bind:value={confirmation} />
    </Field>
    <!-- Screen readers hear what is missing when the button is pressed; the fields show it too. -->
    <p class="visually-hidden" role="alert">{tried ? usernameError || passwordError || confirmationError : ''}</p>
    {#if error}<p class="notice danger" role="alert">{error}</p>{/if}
    <Button type="submit" variant="primary" {busy}>Create account</Button>
  </form>
</AuthLayout>

<style>
  /* A field with a message under it is marked in the same red. */
  .input[aria-invalid='true'] {
    border-color: var(--danger);
  }
</style>
