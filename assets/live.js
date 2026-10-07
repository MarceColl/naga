// naga live: the page shows what the server sends (morphed in place); events go back.
(() => {
  const root = document.getElementById("naga");
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/naga/live?s=${root.dataset.s}`);
  // while you type, the field you're in keeps what you typed
  let typing = false;
  ws.onmessage = (m) => {
    const next = root.cloneNode(false);
    next.innerHTML = m.data;
    morphdom(root, next, {
      childrenOnly: true,
      onBeforeElUpdated: (from) => !(typing && from === document.activeElement),
    });
  };
  // the session ends with the socket: a new page starts a new one
  ws.onclose = () => setTimeout(() => location.reload(), 1000);
  const send = (e) => ws.send(JSON.stringify(e));
  document.addEventListener("click", (e) => {
    const el = e.target.closest("[data-naga-click]");
    if (!el) return;
    e.preventDefault();
    send({ name: el.dataset.nagaClick });
  });
  document.addEventListener("submit", (e) => {
    const el = e.target.closest("[data-naga-submit]");
    if (!el) return;
    e.preventDefault();
    typing = false;
    send({ name: el.dataset.nagaSubmit, form: Object.fromEntries(new FormData(el)) });
  });
  document.addEventListener("input", (e) => {
    const el = e.target.closest("[data-naga-input]");
    if (!el) return;
    typing = true;
    send({ name: el.dataset.nagaInput, value: el.value });
  });
})();
