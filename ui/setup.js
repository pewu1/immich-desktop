'use strict';
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
    setupStatus.textContent = 'Opening Immich…';
    await window.__TAURI__.core.invoke('open_immich');
  } catch (error) { setupStatus.textContent = String(error); }
});
document.querySelector('#open-immich').addEventListener('click', async () => {
  setupStatus.textContent = 'Opening Immich…';
  try {
    await window.__TAURI__.core.invoke('open_immich');
    setupStatus.textContent = 'Immich opened. Drag a photo thumbnail; its original downloads during the same gesture.';
  } catch (error) { setupStatus.textContent = String(error); }
});
