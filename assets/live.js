// naga live: the page shows what the server sends (morphed in place); events go back.
(() => {
  const root = document.getElementById("naga");
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/naga/live?s=${root.dataset.s}`);
  ws.onmessage = (m) => {
    const next = root.cloneNode(false);
    next.innerHTML = m.data;
    morphdom(root, next, { childrenOnly: true });
  };
  // the session ends with the socket: a new page starts a new one
  ws.onclose = () => setTimeout(() => location.reload(), 1000);
  document.addEventListener("click", (e) => {
    const el = e.target.closest("[data-naga-click]");
    if (!el) return;
    e.preventDefault();
    ws.send(JSON.stringify({ name: el.dataset.nagaClick }));
  });
})();
