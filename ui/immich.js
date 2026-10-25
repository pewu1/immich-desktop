// Injected at document start. Rust serializes TRUSTED_ORIGIN; no credentials are read.
'use strict';
if (window.top !== window || location.origin !== TRUSTED_ORIGIN) return;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
let down;
let busy = false;
let suppressClickUntil = 0;
const pending = new Map();
function identify(target) {
  if (!(target instanceof Element) || target.closest('button, input, textarea, select, [contenteditable="true"]')) return;
  const element = target.closest('[data-asset]');
  const id = element?.getAttribute('data-asset');
  if (!UUID.test(id || '') || element.hasAttribute('data-disabled')) return;
  return { id: id.toLowerCase(), element };
}
function bridge() {
  const core = window.__TAURI__?.core;
  if (!core) throw new Error('Native bridge unavailable for this page.');
  return core;
}
function warm(id) {
  if (pending.has(id)) return pending.get(id);
  const task = bridge().invoke('prepare_asset', { assetId: id }).finally(() => pending.delete(id));
  pending.set(id, task); return task;
}
function preview(element) {
  const image = element.querySelector('img');
  if (!image?.complete || !image.naturalWidth || !image.naturalHeight) throw new Error('Wait for the photo thumbnail to load.');
  const scale = Math.min(1, 192 / Math.max(image.naturalWidth, image.naturalHeight));
  const canvas = document.createElement('canvas');
  canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
  canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
  const context = canvas.getContext('2d');
  if (!context) throw new Error('Cannot create the photo drag preview.');
  context.drawImage(image, 0, 0, canvas.width, canvas.height);
  return canvas.toDataURL('image/png');
}
for (const name of ['pointerover', 'focusin']) window.addEventListener(name, event => {
  if (!event.isTrusted) return;
  const photo = identify(event.target);
  if (photo) warm(photo.id).catch(() => {}); // Metadata only; never prefetch originals.
}, true);
window.addEventListener('pointerdown', event => {
  down = undefined;
  if (busy || !event.isTrusted || event.button !== 0 || event.pointerType !== 'mouse' || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
  const photo = identify(event.target); if (!photo) return;
  down = { ...photo, x: event.clientX, y: event.clientY, pointerId: event.pointerId };
  warm(photo.id).catch(() => {});
}, true);
window.addEventListener('pointermove', async event => {
  if (!down || busy || !event.isTrusted || event.pointerId !== down.pointerId || !(event.buttons & 1)) return;
  if (Math.hypot(event.clientX - down.x, event.clientY - down.y) < 8) return;
  const photo = down; down = undefined;
  event.preventDefault(); event.stopImmediatePropagation(); suppressClickUntil = Date.now() + 600;
  busy = true;
  try {
    const core = bridge();
    const previewPng = preview(photo.element);
    const onEvent = new core.Channel();
    onEvent.onmessage = () => {}; // Keep status diagnostics in the standalone drag lab.
    // Do not await an original download. Rust starts a promise/stream while the button is held.
    await core.invoke('drag_asset', { assetId: photo.id, previewPng, onEvent });
  } catch { /* Leave Immich's interface unchanged; a failed gesture can be retried. */ }
  finally { busy = false; }
}, { capture: true, passive: false });
window.addEventListener('dragstart', event => { if (down || identify(event.target)) event.preventDefault(); }, true);
window.addEventListener('click', event => {
  if (Date.now() < suppressClickUntil) { event.preventDefault(); event.stopImmediatePropagation(); }
}, true);
for (const name of ['pointerup', 'pointercancel', 'blur', 'pagehide']) window.addEventListener(name, () => { down = undefined; }, true);
