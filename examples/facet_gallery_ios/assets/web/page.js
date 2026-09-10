// The page's half of the channel. `window.facet.postMessage` is put there by
// the backend; `window.facet.receive` is the page's to define.
window.facet = window.facet || {};
window.facet.receive = function (m) {
  document.getElementById('in').textContent = 'received: ' + m;
};
function postUp() { window.facet.postMessage('hello from the page'); }
