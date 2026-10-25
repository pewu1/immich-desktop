'use strict';
const button = document.querySelector('#reset');
const status = document.querySelector('#reset-status');
button.addEventListener('click', async () => {
  button.disabled = true;
  status.textContent = 'Clearing configuration and login data…';
  try {
    await window.__TAURI__.core.invoke('reset_configuration');
    status.textContent = 'Restarting…';
  } catch (error) {
    status.textContent = String(error);
    button.disabled = false;
  }
});
