// The play page's entry (docs/webrtc.md, "Play in a TV browser"): built by vite.play.config.ts
// into dist/play, which the host serves under /play/ on the guest port with a CSP of its own.
import { mount } from 'svelte';
import '../styles/tokens.css';
import '../styles/base.css';
import Play from './Play.svelte';

const target = document.getElementById('app');
if (!target) throw new Error('missing #app');
export default mount(Play, { target });
