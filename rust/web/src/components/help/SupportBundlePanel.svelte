<script lang="ts">
  // Help's support bundle: Rubylight's logs and the latest crash report in one download.
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import { api } from '../../lib/api';
</script>

<Panel title="Support bundle" level={3} description="Rubylight’s logs and the latest crash report in one file, for a bug report.">
  <div class="row">
    <Button href={api.logs.supportBundleUrl} icon="download">Download support bundle</Button>
  </div>
</Panel>
