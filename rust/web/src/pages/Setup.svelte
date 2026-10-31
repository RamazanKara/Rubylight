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

  const mismatch = $derived(confirmation.length > 0 && confirmation !== password);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (password.length < 8 || mismatch) return;
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
  <form onsubmit={submit}>
    <h1>Set up this host</h1>
    <p class="muted">
      Choose a username and password for this console, here on this PC. Afterwards you can sign in from any
      device on your network to manage Rubylight.
    </p>
    <Field label="Username" id="new-username">
      <input id="new-username" class="input" autocomplete="username" required bind:value={username} />
    </Field>
    <Field label="Password" id="new-password" hint="At least 8 characters.">
      <input id="new-password" class="input" type="password" autocomplete="new-password" minlength="8" required bind:value={password} />
    </Field>
    <Field label="Confirm password" id="confirm-password" error={mismatch ? 'The passwords do not match.' : undefined}>
      <input id="confirm-password" class="input" type="password" autocomplete="new-password" required bind:value={confirmation} />
    </Field>
    {#if error}<p class="notice danger" role="alert">{error}</p>{/if}
    <Button type="submit" variant="primary" {busy} disabled={password.length < 8 || mismatch || !username}>Create account</Button>
  </form>
</AuthLayout>
