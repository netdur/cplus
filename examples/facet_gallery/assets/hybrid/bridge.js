// facet's one page API, the same on every backend: `window.facet.postMessage`
// up, and a `HybridWebViewMessageReceived` event down (`e.detail.message`).
let sent = 0;
document.querySelector('#send').addEventListener('click', () => {
  window.facet.postMessage(`Message from the local page #${++sent}`);
  document.querySelector('#sent').textContent = `${sent} message(s) sent`;
});
window.addEventListener('HybridWebViewMessageReceived', e => {
  document.querySelector('#message').textContent = e.detail.message;
  window.facet.postMessage(`Received: ${e.detail.message}`);
});
