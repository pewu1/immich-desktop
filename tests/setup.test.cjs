const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function page(script, handler) {
  const nodes = new Map();
  const calls = [];
  const node = id => {
    if (!nodes.has(id)) nodes.set(id, { value: '', textContent: '', disabled: false,
      addEventListener(type, callback) { this[type] = callback; } });
    return nodes.get(id);
  };
  vm.runInNewContext(fs.readFileSync(`ui/${script}`, 'utf8'), {
    document: { querySelector: node },
    window: { __TAURI__: { core: { invoke(command, args) {
      calls.push({ command, args });
      return handler(command, args);
    } } } },
  });
  return { node, calls };
}

test('setup restores only the saved server and opens Immich after saving', async () => {
  const h = page('setup.js', command => Promise.resolve(command === 'get_settings' ? { server: 'https://photos.example' } : undefined));
  await Promise.resolve();
  assert.equal(h.node('#server').value, 'https://photos.example');
  assert.equal(h.node('#api-key').value, '');
  h.node('#api-key').value = 'test-key';
  await h.node('#setup').submit({ preventDefault() {} });
  assert.deepEqual(h.calls.map(c => c.command), ['get_settings', 'save_settings', 'open_immich']);
  assert.equal(h.node('#api-key').value, '');
});

test('a failed configuration save stays in setup without opening Immich', async () => {
  const h = page('setup.js', command => command === 'save_settings' ? Promise.reject('Credential store unavailable') : Promise.resolve({ server: '' }));
  await h.node('#setup').submit({ preventDefault() {} });
  assert.equal(h.calls.some(c => c.command === 'open_immich'), false);
  assert.equal(h.node('#setup-status').textContent, 'Credential store unavailable');
});

test('reset requires a click and allows retry when the backend refuses an active transfer', async () => {
  const h = page('reset.js', () => Promise.reject('Wait for the active file transfer to finish before resetting'));
  assert.deepEqual(h.calls, []);
  await h.node('#reset').click();
  assert.equal(h.calls[0].command, 'reset_configuration');
  assert.equal(h.node('#reset').disabled, false);
  assert.match(h.node('#reset-status').textContent, /active file transfer/);
});
