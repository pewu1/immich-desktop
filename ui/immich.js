// Injected at document start. Rust serializes TRUSTED_ORIGIN; no credentials are read.
'use strict';
if (window.top !== window || location.origin !== TRUSTED_ORIGIN) return;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
let down;
let busy = false;
let consumed = false;
let activeGroup;
let selectionManager;
let discovery;
const inspectedModules = new Set();
const pending = new Map();
const posters = new Map();
const INTERACTIVE = 'button, input, textarea, select, [contenteditable="true"], [data-overlay-interactive], [role="slider"], [role="menuitem"], .crop-area, .crop-frame, media-control-bar, media-settings-menu, immich-time-range';
function identify(target) {
  if (!(target instanceof Element) || target.closest(INTERACTIVE)) return;
  // Media Chrome controls are custom elements, often inside a shadow tree.
  if (target.closest('media-play-button, media-time-display, media-volume-range, media-mute-button, media-fullscreen-button, media-settings-menu-button')) return;
  const element = target.closest('[data-asset]');
  const id = element?.getAttribute('data-asset');
  if (UUID.test(id || '') && !element.hasAttribute('data-disabled')) return { id: id.toLowerCase(), element };
  const content = target.closest('#immich-asset-viewer [data-viewer-content]');
  const viewerId = location.pathname?.match(/\/photos\/([^/]+)\/?$/)?.[1];
  if (!content || !UUID.test(viewerId || '')) return;
  if (content.querySelector('.crop-area') || target.closest('a[href]')) return;
  // Immich uses pointer-events-auto for both image layers and interactive
  // overlays. Let the media itself drag while preserving overlay interactions.
  const pointerSurface = target.closest('.pointer-events-auto');
  if (pointerSurface && !pointerSurface.matches('img, video, hls-video')) return;
  // Zoom-image transforms the media wrapper. Ignore AdaptiveImage's internal
  // raster counter-scale (< 1); a zoom transform > 1 belongs to mouse panning.
  for (const start of [target, media(content)]) {
    for (let node = start; node instanceof Element && node !== content; node = node.parentElement) {
      const style = getComputedStyle(node);
      const transform = new DOMMatrixReadOnly(style.transform === 'none' ? undefined : style.transform);
      if (Math.hypot(transform.a, transform.b) > 1.01 || parseFloat(style.scale) > 1.01) return;
    }
  }
  return { id: viewerId.toLowerCase(), element: content, viewer: true };
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
function media(element) {
  const images = [...element.querySelectorAll('img')].filter(image => image.complete && image.naturalWidth && image.naturalHeight && image.getClientRects().length);
  if (images.length) return images.at(-1);
  const video = element.querySelector('video, hls-video');
  const frame = video?.videoWidth ? video : video?.shadowRoot?.querySelector('video');
  if (frame?.readyState >= 2 && frame.videoWidth && frame.videoHeight) return frame;
  const poster = video?.getAttribute('poster');
  if (poster && !posters.has(poster)) {
    const image = new Image(); image.src = poster;
    posters.set(poster, image);
    if (posters.size > 16) posters.delete(posters.keys().next().value);
  }
  const image = posters.get(poster);
  if (image?.complete && image.naturalWidth) return image;
}
function preview(element, count) {
  const image = media(element);
  const width = image?.naturalWidth || image?.videoWidth;
  const height = image?.naturalHeight || image?.videoHeight;
  if (!width || !height) throw new Error('Wait for the media preview to load.');
  const scale = Math.min(1, 192 / Math.max(width, height));
  const canvas = document.createElement('canvas');
  canvas.width = Math.max(1, Math.round(width * scale));
  canvas.height = Math.max(1, Math.round(height * scale));
  const context = canvas.getContext('2d');
  if (!context) throw new Error('Cannot create the photo drag preview.');
  context.drawImage(image, 0, 0, canvas.width, canvas.height);
  if (count > 1) {
    context.font = 'bold 16px sans-serif';
    const label = String(count), badgeWidth = Math.ceil(context.measureText(label).width) + 16;
    context.fillStyle = '#222'; context.fillRect(canvas.width - badgeWidth, 0, badgeWidth, 26);
    context.fillStyle = '#fff'; context.fillText(label, canvas.width - badgeWidth + 8, 19);
  }
  return canvas.toDataURL('image/png');
}
function isSelectionManager(value) {
  return value && typeof value === 'object' && typeof value.hasSelectedAsset === 'function'
    && typeof value.removeAssetFromMultiselectGroup === 'function' && Array.isArray(value.assets);
}
async function discoverSelection() {
  if (selectionManager) return selectionManager;
  if (discovery) return discovery;
  discovery = (async () => {
    // Use the deployed build's exact module URLs, never a second copy of Svelte
    // or guessed chunk names. Only import modules containing the manager API.
    const urls = [...performance.getEntriesByType('resource').map(entry => entry.name),
      ...[...document.querySelectorAll('script[src], link[rel="modulepreload"][href]')].map(node => node.src || node.href)];
    const candidates = [...new Set(urls)].filter(name => {
      const url = new URL(name, location.href);
      return url.origin === TRUSTED_ORIGIN && /\/_app\/immutable\/(chunks|nodes)\/[^/]+\.js$/.test(url.pathname) && !inspectedModules.has(url.href);
    }).sort((a, b) => Number(b.includes('asset-multi-select')) - Number(a.includes('asset-multi-select')));
    for (const url of candidates) {
      try {
        const response = await fetch(url, { cache: 'force-cache' });
        if (!response.ok) continue;
        const source = await response.text();
        inspectedModules.add(url);
        if (!source.includes('hasSelectedAsset') || !source.includes('removeAssetFromMultiselectGroup')) continue;
        const exports = await import(url);
        const managers = Object.values(exports).filter(isSelectionManager);
        if (managers.length === 1) { selectionManager = managers[0]; return selectionManager; }
      } catch { inspectedModules.delete(url); /* Allow retry after a transient load failure. */ }
    }
    throw new Error('This Immich build does not expose a compatible complete selection.');
  })().finally(() => { discovery = undefined; });
  return discovery;
}
async function selectedIds(photo) {
  if (photo.viewer || !photo.element.hasAttribute('data-selected')) return [photo.id];
  const manager = await discoverSelection();
  const ids = [...new Set(manager.assets.map(asset => asset.id?.toLowerCase()))];
  const rendered = [...document.querySelectorAll('#asset-grid [data-asset][data-selected]:not([data-disabled])')];
  if (!ids.length || ids.some(id => !UUID.test(id || '')) || !ids.includes(photo.id)
    || rendered.some(element => !ids.includes(element.getAttribute('data-asset')?.toLowerCase()))) {
    throw new Error('Immich selection changed or its selection adapter is incompatible.');
  }
  return ids;
}
function cancelGroup() {
  if (!activeGroup) return;
  const group = activeGroup; activeGroup = undefined;
  group.released = true;
  if (group.requested) bridge().invoke('cancel_drag', { gestureId: group.id }).catch(() => {});
}
for (const name of ['pointerover', 'focusin']) window.addEventListener(name, event => {
  if (!event.isTrusted) return;
  const photo = identify(event.target);
  if (photo) {
    warm(photo.id).catch(() => {}); // Metadata only; never prefetch originals.
    media(photo.element); // May load the video's existing poster, never its original.
    if (!photo.viewer && photo.element.hasAttribute('data-selected')) discoverSelection().catch(() => {});
  }
}, true);
window.addEventListener('pointerdown', event => {
  down = undefined;
  if (event.isTrusted) { consumed = false; cancelGroup(); }
  if (busy || !event.isTrusted || event.button !== 0 || event.pointerType !== 'mouse' || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
  const photo = identify(event.target); if (!photo) return;
  // Immich installs a 350ms long-press timer on thumbnail pointerdown. Stop
  // that press reaching it, while allowing the browser's ordinary click.
  event.stopImmediatePropagation();
  down = { ...photo, x: event.clientX, y: event.clientY, pointerId: event.pointerId };
  media(photo.element);
  warm(photo.id).catch(() => {});
}, true);
window.addEventListener('pointermove', async event => {
  if (!down || busy || !event.isTrusted || event.pointerId !== down.pointerId || !(event.buttons & 1)) return;
  if (Math.hypot(event.clientX - down.x, event.clientY - down.y) < 8) return;
  const photo = down; down = undefined;
  event.preventDefault(); event.stopImmediatePropagation(); consumed = true;
  busy = true;
  const group = { id: crypto.randomUUID(), released: false, requested: false };
  activeGroup = group;
  try {
    const core = bridge();
    const ids = await selectedIds(photo);
    if (group.released) return;
    const previewPng = preview(photo.element, ids.length);
    const onEvent = new core.Channel();
    onEvent.onmessage = () => {}; // Keep status diagnostics in the standalone drag lab.
    // Singles use deferred delivery; groups prepare retained files while held.
    group.requested = true;
    if (ids.length === 1) await core.invoke('drag_asset', { assetId: ids[0], gestureId: group.id, previewPng, onEvent });
    else {
      await core.invoke('drag_assets', { assetIds: ids, gestureId: group.id, previewPng, onEvent });
    }
  } catch (error) { console.debug('Immich Desktop drag skipped:', error.message); }
  finally { if (activeGroup === group) activeGroup = undefined; busy = false; }
}, { capture: true, passive: false });
window.addEventListener('dragstart', event => { if (down || identify(event.target)) event.preventDefault(); }, true);
window.addEventListener('click', event => {
  if (consumed && event.isTrusted) { event.preventDefault(); event.stopImmediatePropagation(); }
}, true);
for (const name of ['pointerup', 'pointercancel', 'blur', 'pagehide']) window.addEventListener(name, () => { down = undefined; cancelGroup(); }, true);
window.addEventListener('keydown', event => {
  if (event.isTrusted && event.key === 'Escape' && (down || activeGroup)) {
    event.preventDefault(); event.stopImmediatePropagation();
    consumed = true; down = undefined; cancelGroup();
  }
}, true);
