'use strict';
const zone = document.querySelector('#zone');
const results = document.querySelector('#results');
zone.addEventListener('dragover', event => { event.preventDefault(); event.dataTransfer.dropEffect = 'copy'; });
zone.addEventListener('drop', async event => {
  event.preventDefault();
  const files = [...event.dataTransfer.files];
  const types = [...event.dataTransfer.types];
  results.className = '';
  if (!files.length) {
    results.className = 'fail';
    results.textContent = `FAIL: DataTransfer.files is empty. Types: ${types.join(', ')}`;
    return;
  }
  results.textContent = 'Reading dropped files…';
  const fixture = await fetch('../fixtures/fixture.json').then(r => r.json()).catch(() => null);
  const reports = [];
  for (const file of files) {
    const bytes = await file.arrayBuffer();
    const hash = [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(b => b.toString(16).padStart(2, '0')).join('');
    const expected = document.querySelector('#expected').value.trim().toLowerCase() || (file.name === fixture?.name ? fixture.sha256 : '');
    const fixtureFile = file.name === fixture?.name;
    const metadataMatches = fixtureFile ? file.size === fixture.size && file.type === fixture.type : null;
    reports.push({ name: file.name, type: file.type, size: file.size, sha256: hash, bytesMatch: expected ? expected === hash : 'No source hash supplied', fixtureMetadataMatches: metadataMatches, receivedThrough: 'DataTransfer.files' });
  }
  const pass = reports.every(r => r.bytesMatch === true && r.fixtureMetadataMatches !== false);
  results.className = pass ? 'pass' : '';
  results.textContent = JSON.stringify({ time: new Date().toISOString(), browser: navigator.userAgent, types, files: reports }, null, 2);
});
