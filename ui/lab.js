'use strict';
const status = document.querySelector('#status');
let down;
const image = document.querySelector('#fixture');
image.addEventListener('pointerdown', event => {
  if (event.button === 0 && event.isTrusted) down = { x: event.clientX, y: event.clientY };
});
window.addEventListener('pointerup', () => { down = undefined; });
window.addEventListener('pointercancel', () => { down = undefined; });
window.addEventListener('blur', () => { down = undefined; });
window.addEventListener('pointermove', async event => {
  if (!down || !event.isTrusted || !(event.buttons & 1)) return;
  if (Math.hypot(event.clientX - down.x, event.clientY - down.y) < 8) return;
  down = undefined;
  event.preventDefault();
  status.textContent = 'Starting native file drag…';
  try {
    const onEvent = new window.__TAURI__.core.Channel();
    onEvent.onmessage = event => { status.textContent = event.message; };
    await window.__TAURI__.core.invoke('drag_test_file', { onEvent, delayMs: Number(document.querySelector('#lab-delay').value), fresh: document.querySelector('#fresh').checked });
  } catch (error) { status.textContent = String(error); }
}, { passive: false });
image.addEventListener('dragstart', event => event.preventDefault());
fetch('fixture.json').then(r => r.json()).then(data => {
  document.querySelector('#expected').textContent = JSON.stringify(data, null, 2);
});
