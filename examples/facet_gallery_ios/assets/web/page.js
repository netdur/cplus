// The page's half of the channel — facet's one page API, the same on every
// backend: `window.facet.postMessage` up (put there by the backend), and a
// `HybridWebViewMessageReceived` event down, the text in `e.detail.message`.
window.addEventListener('HybridWebViewMessageReceived', function (e) {
  document.getElementById('in').textContent = 'received: ' + e.detail.message;
});
function postUp() { window.facet.postMessage('hello from the page'); }
