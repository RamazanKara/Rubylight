// The guest page's entry (docs/browser-guests.md): built by vite.guest.config.ts into
// dist/guest, which the host serves under /guest/ with a CSP of its own.
import { mount } from 'svelte';
import '../styles/tokens.css';
import '../styles/base.css';
import '../styles/forms.css';
import Guest from './Guest.svelte';

const target = document.getElementById('app');
if (!target) throw new Error('missing #app');
export default mount(Guest, { target });
