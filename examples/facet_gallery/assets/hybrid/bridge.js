window.facet = window.facet || {};
function send(body) {
  if (window.facet.postMessage) window.facet.postMessage(body);
  else if (window.webkit?.messageHandlers?.facet) window.webkit.messageHandlers.facet.postMessage(body);
}
let sent = 0;
document.querySelector('#send').addEventListener('click', () => {
  send(`Message from the local page #${++sent}`);
  document.querySelector('#sent').textContent = `${sent} message(s) sent`;
});
window.facet.onmessage = body => {
  document.querySelector('#message').textContent = body;
  send(`Received: ${body}`);
};
