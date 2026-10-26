const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const code = fs.readFileSync('ui/immich.js', 'utf8');
const id = '11111111-1111-4111-8111-111111111111';
const png = 'data:image/png;base64,preview';
function harness(origin = 'https://photos.example', framed = false, handler) {
  const listeners = new Map(), calls = [], messages = [], drawings = [], rendered = [];
  let manager, modules = [], moduleLoads = 0;
  const page = { origin, href: `${origin}/photos`, pathname: '/photos' };
  class Element {
    constructor(assetId = id, button = false) { this.assetId = assetId; this.button = button; this.loaded = true; this.selected = false; }
    matches(selector) {
      return selector.split(',').some(part => {
        const value = part.trim();
        if (value === '.pointer-events-auto') return this.overlay || this.pointerEventsAuto;
        if (value === 'a[href]') return this.link;
        return value === (this.tagName || 'div');
      });
    }
    closest(selector) {
      if (selector === '[data-asset]') return !this.viewer && this.assetId ? this : this.parentElement?.closest(selector) || null;
      if (selector === '#immich-asset-viewer [data-viewer-content]') return this.viewer ? this : this.parentElement?.closest(selector) || null;
      if (this.matches(selector) || (this.button && selector.includes('button'))) return this;
      return this.parentElement?.closest(selector) || null;
    }
    getAttribute(name) { return name === 'data-asset' ? this.assetId : null; }
    hasAttribute(name) { return name === 'data-selected' && this.selected; }
    querySelectorAll() { return this.noImage ? [] : [this.image || { complete: this.loaded, naturalWidth: this.loaded ? 1000 : 0, naturalHeight: 500, getClientRects() { return [1]; } }]; }
    querySelector(selector) { return selector === '.crop-area' ? this.editor || null : this.video || null; }
  }
  const window = {
    addEventListener(name, fn) { listeners.set(name, fn); },
    __TAURI__: { core: {
      invoke(command, args) { calls.push({ command, args }); return handler?.(command) || Promise.resolve(); },
      Channel: class { onmessage() {} },
    } },
  };
  window.top = framed ? {} : window;
  // Resolve dynamic imports through the deployed-module adapter, without asking
  // Node's VM to execute a remote Svelte build.
  const testCode = code.replace('await import(url)', 'await loadModule(url)');
  vm.runInNewContext(`(() => {const TRUSTED_ORIGIN = 'https://photos.example'; ${testCode}})()`, {
    window, location: page, Element, Date, Map, Promise, URL, console: { debug() {} },
    crypto: { randomUUID: () => 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' },
    getComputedStyle: node => ({ transform: 'none', scale: node.zoom || 'none' }),
    DOMMatrixReadOnly: class { constructor() { this.a = 1; this.b = 0; } },
    performance: { getEntriesByType: () => modules.map(name => ({ name })) },
    fetch: async () => ({ ok: true, text: async () => 'hasSelectedAsset removeAssetFromMultiselectGroup' }),
    loadModule: async () => { moduleLoads++; return { selection: manager }; },
    Image: class { constructor() { this.complete = true; this.naturalWidth = 640; this.naturalHeight = 360; } },
    document: { querySelectorAll: () => rendered, body: { append() {} }, createElement(tag) {
      if (tag === 'canvas') return { width: 0, height: 0, getContext() { return { drawImage(...args) { drawings.push(args); }, measureText: () => ({ width: 10 }), fillRect() {}, fillText() {} }; }, toDataURL() { return png; } };
      return { style: {}, setAttribute() {}, set textContent(s) { messages.push(s); } };
    } },
  });
  const event = (target = new Element(), extra = {}) => ({ target, isTrusted: true, button: 0, buttons: 1, pointerType: 'mouse', pointerId: 1, clientX: 0, clientY: 0, preventDefault() { this.prevented = true; }, stopImmediatePropagation() { this.stopped = true; }, ...extra });
  const selection = ids => {
    manager = { assets: ids.map(id => ({ id })), hasSelectedAsset: value => ids.includes(value), removeAssetFromMultiselectGroup() {} };
    modules = ['https://photos.example/_app/immutable/chunks/selection.js'];
    return manager;
  };
  return { listeners, calls, messages, drawings, Element, event, selection, rendered, page, get moduleLoads() { return moduleLoads; } };
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
  for (const options of [{ shiftKey: true }, { ctrlKey: true }, { metaKey: true }, { altKey: true }, { isTrusted: false }, { button: 2 }, { pointerType: 'touch' }]) {
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

test('a single dragged item stays unchecked after a long press and a late release click', async () => {
  const h = harness(), photo = new h.Element();
  // Model Immich's target-phase 350ms long-press selection timer. Its handler
  // must never receive an eligible desktop press, even before the threshold.
  let longPress;
  const press = h.event(photo);
  h.listeners.get('pointerdown')(press);
  if (!press.stopped) longPress = () => { photo.selected = true; };
  longPress?.(); // A user can hold still before beginning the movement.
  assert.equal(photo.selected, false);
  await h.listeners.get('pointermove')(h.event(photo, { clientX: 12 }));
  h.listeners.get('pointerup')();
  const click = h.event(photo, { timeStamp: 5000 });
  h.listeners.get('click')(click);
  if (!click.stopped) photo.selected = !photo.selected;
  assert.equal(photo.selected, false);
  assert.equal(click.prevented, true);
  // A new ordinary press/click must not inherit drag click suppression.
  h.listeners.get('pointerdown')(h.event(photo));
  h.listeners.get('pointerup')();
  const nextClick = h.event(photo); h.listeners.get('click')(nextClick);
  assert.equal(nextClick.stopped, undefined);
});

test('failed drags and cancellation also consume the gesture click without selecting', async () => {
  for (const failure of ['preview', 'native']) {
    const h = harness(undefined, false, command => command === 'drag_asset' ? Promise.reject(new Error('failed')) : Promise.resolve());
    const photo = new h.Element(); photo.loaded = failure !== 'preview';
    h.listeners.get('pointerdown')(h.event(photo));
    await h.listeners.get('pointermove')(h.event(photo, { clientX: 12 }));
    h.listeners.get('pointercancel')();
    const click = h.event(photo); h.listeners.get('click')(click);
    assert.equal(click.stopped, true); assert.equal(photo.selected, false);
  }
});

const otherId = '22222222-2222-4222-8222-222222222222';
function viewerMedia(h, tagName = 'img') {
  const content = new h.Element(), wrapper = new h.Element(null), image = new h.Element(null);
  content.viewer = true; h.page.pathname = `/photos/${id}`;
  wrapper.parentElement = content; image.parentElement = wrapper;
  image.tagName = tagName; image.pointerEventsAuto = true;
  image.complete = true; image.naturalWidth = 800; image.naturalHeight = 1200;
  image.getClientRects = () => [1]; content.image = image;
  if (tagName !== 'img') {
    content.noImage = true; content.video = image;
    image.readyState = 2; image.videoWidth = 800; image.videoHeight = 1200;
  }
  return { content, wrapper, image };
}
test('checked thumbnail drags the complete mixed selection including unrendered assets', async () => {
  const h = harness(), photo = new h.Element(); photo.selected = true;
  const manager = h.selection([id, otherId]); h.rendered.push(photo);
  h.listeners.get('pointerdown')(h.event(photo));
  await h.listeners.get('pointermove')(h.event(photo, { clientX: 12 }));
  const drag = h.calls.find(call => call.command === 'drag_assets');
  assert.deepEqual(Array.from(drag.args.assetIds), [id, otherId]);
  assert.equal(h.moduleLoads, 1); assert.equal(photo.selected, true); assert.equal(manager.assets.length, 2);
});
test('unchecked thumbnails drag alone while other items remain checked', async () => {
  const h = harness(), photo = new h.Element(); h.selection([otherId]);
  h.listeners.get('pointerdown')(h.event(photo));
  await h.listeners.get('pointermove')(h.event(photo, { clientX: 12 }));
  assert.equal(h.calls.find(call => call.command === 'drag_asset').args.assetId, id);
  assert.equal(h.moduleLoads, 0); assert.equal(photo.selected, false);
});
test('an incompatible full-selection adapter never silently exports a subset', async () => {
  const h = harness(), photo = new h.Element(); photo.selected = true;
  h.selection([otherId]);
  h.listeners.get('pointerdown')(h.event(photo));
  await h.listeners.get('pointermove')(h.event(photo, { clientX: 12 }));
  assert.equal(h.calls.some(call => call.command.startsWith('drag_')), false);
});
test('release during group preparation cancels the exact native request', async () => {
  let finish;
  const h = harness(undefined, false, command => command === 'drag_assets' ? new Promise(resolve => { finish = resolve; }) : Promise.resolve());
  const photo = new h.Element(); photo.selected = true; h.selection([id, otherId]);
  h.listeners.get('pointerdown')(h.event(photo));
  const movement = h.listeners.get('pointermove')(h.event(photo, { clientX: 12 }));
  for (let index = 0; index < 20 && !finish; index++) await Promise.resolve();
  assert.ok(finish);
  h.listeners.get('pointerup')();
  assert.equal(h.calls.find(call => call.command === 'cancel_drag').args.gestureId, h.calls.find(call => call.command === 'drag_assets').args.gestureId);
  finish(); await movement;
});
test('Escape cancels preparation without reaching Immich selection clearing', async () => {
  let finish;
  const h = harness(undefined, false, command => command === 'drag_assets' ? new Promise(resolve => { finish = resolve; }) : Promise.resolve());
  const photo = new h.Element(); photo.selected = true; const manager = h.selection([id, otherId]);
  h.listeners.get('pointerdown')(h.event(photo));
  const movement = h.listeners.get('pointermove')(h.event(photo, { clientX: 12 }));
  for (let index = 0; index < 20 && !finish; index++) await Promise.resolve();
  assert.ok(finish);
  const escape = h.event(photo, { key: 'Escape' }); h.listeners.get('keydown')(escape);
  if (!escape.stopped) manager.assets = [];
  assert.equal(manager.assets.length, 2); assert.equal(escape.prevented, true);
  assert.ok(h.calls.some(call => call.command === 'cancel_drag'));
  finish(); await movement;
});
test('opened photo and video drag the current asset even with checked timeline items', async () => {
  for (const video of [false, true]) {
    const h = harness(), content = new h.Element(); content.viewer = true;
    h.page.pathname = `/albums/${otherId}/photos/${id}`; h.selection([otherId]);
    if (video) { content.noImage = true; content.video = { readyState: 2, videoWidth: 1280, videoHeight: 720 }; }
    h.listeners.get('pointerdown')(h.event(content));
    await h.listeners.get('pointermove')(h.event(content, { clientX: 12 }));
    assert.equal(h.calls.find(call => call.command === 'drag_asset').args.assetId, id);
  }
});
test('viewer media with pointer-events-auto and its background each start exactly one drag', async () => {
  for (const tagName of ['img', 'video', 'hls-video']) {
    for (const background of [false, true]) {
      const h = harness(), { content, image } = viewerMedia(h, tagName);
      const target = background ? content : image;
      const press = h.event(target); h.listeners.get('pointerdown')(press);
      assert.equal(press.stopped, true, `${tagName}: eligible viewer press`);
      await h.listeners.get('pointermove')(h.event(target, { clientX: 12 }));
      await h.listeners.get('pointermove')(h.event(target, { clientX: 24 }));
      const drags = h.calls.filter(call => call.command === 'drag_asset');
      assert.equal(drags.length, 1); assert.equal(drags[0].args.assetId, id);
      assert.equal(h.drawings[0][0], image);
      assert.equal(h.drawings[0][3], 128); assert.equal(h.drawings[0][4], 192);
    }
  }
});
test('nested face and OCR overlays, links and controls remain upstream interactions', () => {
  for (const mode of ['face', 'ocr', 'link', 'control']) {
    const h = harness(), { wrapper } = viewerMedia(h);
    const overlay = new h.Element(null), target = new h.Element(null);
    overlay.parentElement = wrapper; target.parentElement = overlay;
    overlay.overlay = mode === 'face' || mode === 'ocr';
    overlay.link = mode === 'link'; overlay.button = mode === 'control';
    const press = h.event(target); h.listeners.get('pointerdown')(press);
    assert.equal(press.stopped, undefined); assert.equal(h.calls.length, 0);
  }
});
test('pointer-events-auto media and background retain panning through a zoomed wrapper', () => {
  for (const background of [false, true]) {
    const h = harness(), { content, wrapper, image } = viewerMedia(h);
    wrapper.zoom = '2';
    const press = h.event(background ? content : image);
    h.listeners.get('pointerdown')(press);
    assert.equal(press.stopped, undefined); assert.equal(h.calls.length, 0);
  }
});
test('a video poster supplies the preview before its first frame is ready', async () => {
  const h = harness(), content = new h.Element(); content.viewer = true; content.noImage = true;
  content.video = { readyState: 0, getAttribute: () => '/poster.jpg' };
  h.page.pathname = `/photos/${id}`;
  h.listeners.get('pointerdown')(h.event(content));
  await h.listeners.get('pointermove')(h.event(content, { clientX: 12 }));
  assert.equal(h.calls.some(call => call.command === 'drag_asset'), true);
});
test('zoomed viewer surfaces and video controls preserve native interactions', () => {
  const h = harness(), content = new h.Element(), photo = new h.Element(); content.viewer = true;
  photo.viewer = true; photo.closest = selector => selector.includes('[data-viewer-content]') ? content : null;
  photo.parentElement = content; photo.zoom = '2'; h.page.pathname = `/photos/${id}`;
  const pan = h.event(photo); h.listeners.get('pointerdown')(pan);
  assert.equal(pan.stopped, undefined); assert.equal(h.calls.length, 0);
  const control = new h.Element(id, true); control.viewer = true;
  const press = h.event(control); h.listeners.get('pointerdown')(press);
  assert.equal(press.stopped, undefined); assert.equal(h.calls.length, 0);
});
test('viewer background presses still preserve panning when the zoom wrapper is a child', () => {
  const h = harness(), content = new h.Element(), image = new h.Element();
  content.viewer = true; h.page.pathname = `/photos/${id}`;
  const zoomWrapper = new h.Element(); zoomWrapper.zoom = '2'; zoomWrapper.parentElement = content;
  image.complete = true; image.naturalWidth = 1000; image.naturalHeight = 500;
  image.getClientRects = () => [1]; image.parentElement = zoomWrapper; content.image = image;
  const press = h.event(content); h.listeners.get('pointerdown')(press);
  assert.equal(press.stopped, undefined); assert.equal(h.calls.length, 0);
});
test('crop editing and interactive viewer overlays do not start outbound dragging', () => {
  for (const mode of ['editor', 'overlay']) {
    const h = harness(), content = new h.Element(); content.viewer = true;
    content[mode] = true; h.page.pathname = `/photos/${id}`;
    const press = h.event(content); h.listeners.get('pointerdown')(press);
    assert.equal(press.stopped, undefined); assert.equal(h.calls.length, 0);
  }
});
