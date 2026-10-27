import { mount } from 'svelte';
import '../styles/tokens.css';
import '../styles/base.css';
import '../styles/forms.css';
import Gallery from './Gallery.svelte';

const target = document.getElementById('app');
if (!target) throw new Error('missing #app');
export default mount(Gallery, { target });
