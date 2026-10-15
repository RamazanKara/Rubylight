const table = document.getElementById('results');

if (table) {
  const body = table.tBodies[0];
  const status = document.getElementById('results-status');
  const filter = document.getElementById('codec-filter');
  const headers = [...table.tHead.rows[0].cells];
  const collator = new Intl.Collator('en', { numeric: true, sensitivity: 'base' });
  let rows = [];
  let sortKey = 'soc';
  let ascending = true;

  const joined = (...parts) => parts.filter(Boolean).join(' ');
  const deviceName = entry => {
    const device = entry.results.device || {};
    return entry.name || joined(device.manufacturer, device.model) || 'Unknown device';
  };
  const socName = device => joined(device.socManufacturer, device.socModel) || device.hardware || '';
  const lowLatency = decoder => {
    const keys = Array.isArray(decoder.vendorLowLatencyKeys) ? decoder.vendorLowLatencyKeys.length : null;
    const parts = [];
    if (decoder.featureLowLatency === true) parts.push('Android');
    if (keys) parts.push(keys === 1 ? '1 vendor key' : keys + ' vendor keys');
    if (parts.length) return parts.join(', ');
    return decoder.featureLowLatency === false && keys === 0 ? 'None' : '';
  };

  const flatten = data => {
    const out = [];
    for (const entry of data.entries || []) {
      const results = entry.results || {};
      const device = results.device || {};
      const benchmark = results.decoderBenchmark || {};
      for (const decoder of benchmark.measured ? benchmark.results || [] : []) {
        if (typeof decoder.medianDecodeMs !== 'number') continue;
        out.push({
          device: deviceName(entry),
          model: joined(device.manufacturer, device.model),
          soc: socName(device),
          android: device.android || '',
          codec: decoder.codec || '',
          decoder: decoder.decoder || '',
          ms: decoder.medianDecodeMs,
          lowLatency: lowLatency(decoder),
          app: results.app && results.app.version || '',
          issue: Number.isInteger(entry.issue) ? entry.issue : 0
        });
      }
    }
    return out;
  };

  const cell = (row, text, className) => {
    const td = row.insertCell();
    td.textContent = text;
    if (className) td.className = className;
    return td;
  };

  const render = () => {
    const codec = filter.value;
    const shown = rows.filter(row => !codec || row.codec === codec);
    shown.sort((a, b) => {
      const order = sortKey === 'ms' || sortKey === 'issue' ? a[sortKey] - b[sortKey] : collator.compare(a[sortKey], b[sortKey]);
      return (ascending ? order : -order) || collator.compare(a.codec, b.codec) || a.ms - b.ms;
    });
    body.replaceChildren();
    if (!shown.length) {
      const empty = body.insertRow().insertCell();
      empty.colSpan = headers.length;
      empty.className = 'results-empty';
      empty.textContent = rows.length ? 'No results for this codec yet.' : 'No results yet. Measure your device and add it below.';
      return;
    }
    for (const item of shown) {
      const row = body.insertRow();
      const device = cell(row, item.device);
      if (item.model && item.model !== item.device) device.title = item.model;
      cell(row, item.soc || '—');
      cell(row, item.android || '—');
      cell(row, item.codec);
      cell(row, item.decoder || '—', 'code');
      cell(row, item.ms.toFixed(2) + ' ms', 'number');
      cell(row, item.lowLatency || '—');
      cell(row, item.app || '—');
      const source = row.insertCell();
      if (item.issue) {
        const link = document.createElement('a');
        link.href = 'https://github.com/RamazanKara/nightjar/issues/' + item.issue;
        link.textContent = '#' + item.issue;
        source.append(link);
      } else {
        source.textContent = '—';
      }
    }
  };

  for (const header of headers) {
    header.querySelector('button').addEventListener('click', () => {
      const key = header.dataset.key;
      ascending = key === sortKey ? !ascending : true;
      sortKey = key;
      for (const other of headers) other.removeAttribute('aria-sort');
      header.setAttribute('aria-sort', ascending ? 'ascending' : 'descending');
      render();
    });
  }
  filter.addEventListener('change', render);

  fetch('data/device-results.json', { cache: 'no-cache' })
    .then(response => {
      if (!response.ok) throw new Error(response.status);
      return response.json();
    })
    .then(data => {
      rows = flatten(data);
      render();
    })
    .catch(() => {
      status.textContent = 'The results could not be loaded. The raw data is in data/device-results.json.';
    });
}
