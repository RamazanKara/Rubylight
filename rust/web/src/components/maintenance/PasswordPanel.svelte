<script lang="ts">
  import Button from '../Button.svelte';
  import Field from '../Field.svelte';
  import Panel from '../Panel.svelte';
  import { api } from '../../lib/api';
  import { failed, notify } from '../../lib/feedback.svelte';
  import { signedOut } from '../../lib/session.svelte';

  /** The signed-in username, once known, fills in both username fields. */
  let { username }: { username: string } = $props();

  let currentUsername = $state('');
  let currentPassword = $state('');
  let newUsername = $state('');
  let newPassword = $state('');
  let confirmation = $state('');
  let busy = $state(false);

  let prefilled = false;
  $effect(() => {
    if (!username || prefilled) return;
    prefilled = true;
    currentUsername ||= username;
    newUsername ||= username;
  });

  /** Change password was pressed: what is missing shows from then on. */
  let tried = $state(false);
  // The button stays ready; what is missing shows on its field when it is pressed.
  const problems = $derived<[string, string][]>([
    ['pw-current-username', currentUsername.trim() ? '' : 'Enter the current username.'],
    ['pw-current-password', currentPassword ? '' : 'Enter the current password.'],
    ['pw-new-username', newUsername.trim() ? '' : 'Enter a username.'],
    ['pw-new-password', newPassword.length >= 8 ? '' : 'Use at least 8 characters.'],
    ['pw-confirm', confirmation === newPassword ? '' : 'The passwords do not match.'],
  ]);
  const shown = $derived(
    Object.fromEntries(
      problems.map(([id, problem]) => [
        id,
        // A short password or a different confirmation shows as soon as one is typed.
        tried || (id === 'pw-new-password' && newPassword.length > 0) || (id === 'pw-confirm' && confirmation.length > 0) ? problem : '',
      ]),
    ),
  );

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    tried = true;
    const first = problems.find(([, problem]) => problem);
    if (first) {
      document.getElementById(first[0])?.focus();
      return;
    }
    busy = true;
    try {
      await api.auth.setPassword({
        currentUsername: currentUsername.trim(),
        currentPassword,
        newUsername: newUsername.trim(),
        newPassword,
        confirmNewPassword: confirmation,
      });
      notify('Sign-in details changed. Sign in again with the new ones.', 'ok');
      signedOut();
    } catch (error) {
      failed('Changing the sign-in details failed', error);
    } finally {
      busy = false;
    }
  }
</script>

<Panel title="Change password" level={3} description="Saving signs out every browser, including this one.">
  <form class="stack" onsubmit={submit} novalidate>
    <div class="form-grid">
      <Field label="Current username" id="pw-current-username" error={shown['pw-current-username'] || undefined}>
        <input
          id="pw-current-username"
          class="input"
          autocomplete="username"
          required
          aria-invalid={shown['pw-current-username'] ? true : undefined}
          bind:value={currentUsername}
        />
      </Field>
      <Field label="Current password" id="pw-current-password" error={shown['pw-current-password'] || undefined}>
        <input
          id="pw-current-password"
          class="input"
          type="password"
          autocomplete="current-password"
          required
          aria-invalid={shown['pw-current-password'] ? true : undefined}
          bind:value={currentPassword}
        />
      </Field>
      <Field
        label="New username"
        id="pw-new-username"
        hint="Keep it the same to change only the password."
        error={shown['pw-new-username'] || undefined}
        wide
      >
        <input
          id="pw-new-username"
          class="input"
          autocomplete="off"
          required
          aria-invalid={shown['pw-new-username'] ? true : undefined}
          bind:value={newUsername}
        />
      </Field>
      <Field label="New password" id="pw-new-password" hint="At least 8 characters." error={shown['pw-new-password'] || undefined}>
        <input
          id="pw-new-password"
          class="input"
          type="password"
          autocomplete="new-password"
          minlength="8"
          required
          aria-invalid={shown['pw-new-password'] ? true : undefined}
          bind:value={newPassword}
        />
      </Field>
      <Field label="Confirm new password" id="pw-confirm" error={shown['pw-confirm'] || undefined}>
        <input
          id="pw-confirm"
          class="input"
          type="password"
          autocomplete="new-password"
          required
          aria-invalid={shown['pw-confirm'] ? true : undefined}
          bind:value={confirmation}
        />
      </Field>
    </div>
    <!-- Screen readers hear what is missing when the button is pressed; the fields show it too. -->
    <p class="visually-hidden" role="alert">{tried ? (problems.find(([, problem]) => problem)?.[1] ?? '') : ''}</p>
    <div class="row">
      <Button type="submit" {busy}>Change password</Button>
    </div>
  </form>
</Panel>

<style>
  /* A field with a message under it is marked in the same red. */
  .input[aria-invalid='true'] {
    border-color: var(--danger);
  }
</style>
