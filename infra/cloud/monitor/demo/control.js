(() => {
  const canvas = document.querySelector("canvas");
  canvas.tabIndex = 0;
  canvas.style.touchAction = "none";
  const label = document.createElement("span");
  document.querySelector("header").append(label);
  let socket, ready = false;
  function send(message) {
    if (ready) socket.send(message);
  }
  function attach() {
    if (!sessionStorage.viewer) return setTimeout(attach, 500);
    socket = new WebSocket("ws://" + location.host + "/control");
    socket.onopen = () => socket.send(sessionStorage.viewer);
    socket.onmessage = () => { ready = true; label.textContent = "Click desktop to control"; };
    socket.onclose = () => {
      ready = false;
      label.textContent = "Input disconnected";
      setTimeout(attach, 2000);
    };
  }
  function position(e) {
    const r = canvas.getBoundingClientRect();
    const scale = Math.min(r.width / canvas.width, r.height / canvas.height);
    const x = Math.floor((e.clientX-r.left-(r.width-canvas.width*scale)/2)/scale);
    const y = Math.floor((e.clientY-r.top-(r.height-canvas.height*scale)/2)/scale);
    send('move ' + Math.max(0, Math.min(canvas.width-1,x)) + ' ' + Math.max(0, Math.min(canvas.height-1,y)));
  }
  canvas.onpointermove = position;
  canvas.onpointerdown = e => {
    e.preventDefault(); canvas.focus(); canvas.setPointerCapture(e.pointerId);
    position(e); if (e.button <= 2) send('button '+(e.button+1)+' 1');
  };
  canvas.onpointerup = e => { position(e); if(e.button <= 2) send('button '+(e.button+1)+' 0'); };
  canvas.onpointercancel = canvas.onblur = () => send("release");
  canvas.oncontextmenu = e => e.preventDefault();
  canvas.addEventListener("wheel", e => {
    e.preventDefault(); position(e);
    const b = Math.abs(e.deltaY) >= Math.abs(e.deltaX) ? (e.deltaY < 0 ? 4 : 5) : (e.deltaX < 0 ? 6 : 7);
    send('button '+b+' 1'); send('button '+b+' 0');
  }, {passive:false});
  const keys = {Enter:"Return",Backspace:"BackSpace",Escape:"Escape",Tab:"Tab",Space:"space",
    ArrowLeft:"Left",ArrowRight:"Right",ArrowDown:"Down",ArrowUp:"Up",ShiftLeft:"Shift_L",ShiftRight:"Shift_R",
    ControlLeft:"Control_L",ControlRight:"Control_R",AltLeft:"Alt_L",AltRight:"Alt_R",MetaLeft:"Super_L",
    MetaRight:"Super_R",Minus:"minus",Equal:"equal",BracketLeft:"bracketleft",BracketRight:"bracketright",
    Backslash:"backslash",Semicolon:"semicolon",Quote:"apostrophe",Comma:"comma",Period:"period",Slash:"slash",
    Backquote:"grave",Home:"Home",End:"End",Delete:"Delete",PageUp:"Prior",PageDown:"Next"};
  canvas.onkeydown = canvas.onkeyup = e => {
    const key = keys[e.code] || (/^Key[A-Z]$/.test(e.code) ? e.code.slice(3).toLowerCase()
      : /^Digit[0-9]$/.test(e.code) ? e.code.slice(5) : /^F\d{1,2}$/.test(e.code) ? e.code : null);
    if (key) { e.preventDefault(); send('key '+key+' '+(e.type==="keydown" ? 1 : 0)); }
  };
  attach();
})();
