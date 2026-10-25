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

const setupStatus = document.querySelector('#setup-status');
const server = document.querySelector('#server');
const apiKey = document.querySelector('#api-key');
window.__TAURI__.core.invoke('get_settings').then(settings => { server.value = settings.server; }).catch(error => { setupStatus.textContent = String(error); });
document.querySelector('#setup').addEventListener('submit', async event => {
  event.preventDefault();
  setupStatus.textContent = 'Saving to the OS credential store…';
  try {
    await window.__TAURI__.core.invoke('save_settings', { server: server.value, apiKey: apiKey.value });
    apiKey.value = '';
    setupStatus.textContent = 'Saved. Open Immich to log in and browse. Restart before changing configuration after opening Immich.';
  } catch (error) { setupStatus.textContent = String(error); }
});
document.querySelector('#open-immich').addEventListener('click', async () => {
  setupStatus.textContent = 'Opening Immich…';
  try {
    await window.__TAURI__.core.invoke('open_immich');
    setupStatus.textContent = 'Immich opened. Drag a photo thumbnail; its original downloads during the same gesture.';
  } catch (error) { setupStatus.textContent = String(error); }
});
