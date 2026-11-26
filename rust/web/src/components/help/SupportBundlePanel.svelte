<script lang="ts">
  // Help's support bundle: Rubylight's logs and diagnostics in one download, with addresses, names
  // and game titles replaced by marks. The crash memory dump comes only when asked for.
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import Toggle from '../Toggle.svelte';
  import { api } from '../../lib/api';

  let includeDump = $state(false);
</script>

<Panel title="Support bundle" level={3} description="Rubylight’s logs and diagnostics in one file, for a bug report. Addresses, user and device names and game titles are replaced by marks such as [lan-ip] or [app-1].">
  <div class="stack">
    <Toggle
      label="Include the crash memory dump"
      hint="A memory dump can hold anything Rubylight had in memory, such as names, addresses and passwords. Include it only when asked, and share it privately."
      bind:checked={includeDump}
    />
    <div class="row">
      <Button href={includeDump ? api.logs.supportBundleWithDumpUrl : api.logs.supportBundleUrl} icon="download">Download support bundle</Button>
    </div>
  </div>
</Panel>
