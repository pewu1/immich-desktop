const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const code = fs.readFileSync('ui/immich.js', 'utf8');
const id = '11111111-1111-4111-8111-111111111111';
const png = 'data:image/png;base64,preview';
function harness(origin = 'https://photos.example', framed = false, handler) {
  const listeners = new Map(), calls = [], messages = [], drawings = [];
  class Element {
    constructor(assetId = id, button = false) { this.assetId = assetId; this.button = button; this.loaded = true; }
    closest(selector) { return selector === '[data-asset]' ? this : this.button ? this : null; }
    getAttribute() { return this.assetId; }
    hasAttribute() { return false; }
    querySelector() { return { complete: this.loaded, naturalWidth: this.loaded ? 1000 : 0, naturalHeight: 500 }; }
  }
  const window = {
    addEventListener(name, fn) { listeners.set(name, fn); },
    __TAURI__: { core: {
      invoke(command, args) { calls.push({ command, args }); return handler?.(command) || Promise.resolve(); },
      Channel: class { onmessage() {} },
    } },
  };
  window.top = framed ? {} : window;
  vm.runInNewContext(`(() => {const TRUSTED_ORIGIN = 'https://photos.example'; ${code}})()`, {
    window, location: { origin }, Element, Date, Map, Promise,
    document: { body: { append() {} }, createElement(tag) {
      if (tag === 'canvas') return { width: 0, height: 0, getContext() { return { drawImage(...args) { drawings.push(args); } }; }, toDataURL() { return png; } };
      return { style: {}, setAttribute() {}, set textContent(s) { messages.push(s); } };
    } },
  });
  const event = (target = new Element(), extra = {}) => ({ target, isTrusted: true, button: 0, buttons: 1, pointerType: 'mouse', pointerId: 1, clientX: 0, clientY: 0, preventDefault() {}, stopImmediatePropagation() {}, ...extra });
  return { listeners, calls, messages, drawings, Element, event };
}
test('foreign origins and frames never install gestures', () => {
  assert.equal(harness('https://evil.example').listeners.size, 0);
  assert.equal(harness(undefined, true).listeners.size, 0);
});
test('hover and press warm metadata but never request original-file dragging', () => {
  const h = harness();
  h.listeners.get('pointerover')(h.event());
  h.listeners.get('pointerdown')(h.event());
  assert.deepEqual(h.calls.map(c => c.command), ['prepare_asset']);
});
test('one gesture immediately requests deferred drag even with metadata preparation pending', async () => {
  const h = harness(undefined, false, command => command === 'prepare_asset' ? new Promise(() => {}) : Promise.resolve());
  h.listeners.get('pointerdown')(h.event());
  await h.listeners.get('pointermove')(h.event(undefined, { clientX: 20 }));
  await h.listeners.get('pointermove')(h.event(undefined, { clientX: 30 }));
  assert.deepEqual(h.calls.map(c => c.command), ['prepare_asset', 'drag_asset']);
  assert.equal(h.calls[1].args.assetId, id);
  assert.equal(h.calls[1].args.previewPng, png);
  assert.equal(h.drawings[0][3], 192);
  assert.equal(h.drawings[0][4], 96);
  assert.ok(!h.messages.some(m => /drag again/i.test(m)));
});
test('release before threshold prevents any delayed drag', async () => {
  let finish;
  const h = harness(undefined, false, () => new Promise(resolve => { finish = resolve; }));
  h.listeners.get('pointerdown')(h.event()); h.listeners.get('pointerup')(); finish();
  await h.listeners.get('pointermove')(h.event(undefined, { clientX: 20 }));
  assert.deepEqual(h.calls.map(c => c.command), ['prepare_asset']);
});
test('native status events add no toast or text to Immich', async () => {
  const h = harness();
  h.listeners.get('pointerdown')(h.event());
  await h.listeners.get('pointermove')(h.event(undefined, { clientX: 20 }));
  const old = h.calls.find(c => c.command === 'drag_asset').args.onEvent;
  old.onmessage({ stage: 'error', message: 'Disk full' });
  h.listeners.get('pointerdown')(h.event());
  await h.listeners.get('pointermove')(h.event(undefined, { clientX: 20 }));
  old.onmessage({ stage: 'ready', message: 'Old download' });
  assert.deepEqual(h.messages, []);
});
test('unloaded thumbnails produce an error instead of a generic preview or original request', async () => {
  const h = harness(); const image = new h.Element(); image.loaded = false;
  h.listeners.get('pointerdown')(h.event(image));
  await h.listeners.get('pointermove')(h.event(image, { clientX: 20 }));
  assert.deepEqual(h.messages, []);
  assert.equal(h.calls.filter(c => c.command === 'drag_asset').length, 0);
});
test('modifiers, selection controls, synthetic events and short movements preserve browsing', async () => {
  for (const options of [{ shiftKey: true }, { ctrlKey: true }, { metaKey: true }, { altKey: true }, { isTrusted: false }, { button: 2 }]) {
    const h = harness(); h.listeners.get('pointerdown')(h.event(undefined, options)); assert.equal(h.calls.length, 0);
  }
  const h = harness();
  h.listeners.get('pointerdown')(h.event(new h.Element(id, true)));
  h.listeners.get('pointerdown')(h.event(new h.Element('../../etc/passwd')));
  assert.equal(h.calls.length, 0);
  h.listeners.get('pointerdown')(h.event());
  await h.listeners.get('pointermove')(h.event(undefined, { clientX: 3 }));
  assert.deepEqual(h.calls.map(c => c.command), ['prepare_asset']);
});
