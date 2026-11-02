// A real module/fetch/WebSocket page. Captures only a boolean credential-leak
// verdict; request cookies and application credentials are never logged.
import http from 'node:http';
import crypto from 'node:crypto';
import {fork} from 'node:child_process';
import {fileURLToPath} from 'node:url';
async function serve() {
  const record=value=>process.send(value);
  const sockets=new Set();
  const server=http.createServer((request,response)=>{
    record({path:request.url,applicationCookie:(request.headers.cookie||'').includes('zuse_session=')});
    if(request.url==='/data'){response.setHeader('Content-Type','application/json');response.end(JSON.stringify({value:'Fetched from preview'}));return;}
    if(request.url==='/main.js'){
      response.setHeader('Content-Type','text/javascript');
      response.end(`
        let access='blocked';try{void parent.document.body;access='allowed';}catch{}
        document.body.dataset.parentAccess=access;
        document.body.dataset.credentialless=String(window.credentialless);
        const data=await fetch('/data').then(r=>r.json());document.querySelector('#fetch').textContent=data.value;
        const socket=new WebSocket('ws://'+location.host+'/socket');socket.onopen=()=>socket.send('hello');socket.onmessage=e=>{document.querySelector('#socket').textContent=e.data;report();};
        let count=0;
        function rect(selector){const r=document.querySelector(selector).getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2];}
        function report(){parent.postMessage({previewProof:{ready:document.body.dataset.ready,parentAccess:access,credentialless:window.credentialless,fetch:document.querySelector('#fetch').textContent,socket:document.querySelector('#socket').textContent,draft:document.querySelector('#draft').value,count,draftPoint:rect('#draft'),buttonPoint:rect('#increment')}},'*');}
        document.querySelector('#increment').onclick=()=>{document.querySelector('#count').textContent=String(++count);report();};
        document.querySelector('#draft').oninput=report;
        window.addEventListener('message',event=>{if(event.source===parent&&event.data==='preview-inspect')report();});
        document.body.dataset.ready='true';report();
      `);return;
    }
    response.setHeader('Content-Type','text/html');
    response.end(`<!doctype html><meta name="viewport" content="width=device-width"><title>Project preview</title>
      <style>body{font:15px system-ui;padding:24px;color:#223026;background:#f7faf6}h1{font-size:24px}button,input{font:inherit;padding:10px;margin:8px 0}button{background:#83c027;border:0;border-radius:6px}input{width:90%}p{color:#526351}</style>
      <h1>Project preview</h1><p id="fetch">Loading module…</p><p id="socket">Connecting…</p>
      <label>Draft<input id="draft" placeholder="Type inside the preview"></label><button id="increment">Increment</button><p>Count: <span id="count">0</span></p>
      <script type="module" src="/main.js"></script>`);
  });
  server.on('upgrade',(request,socket)=>{
    sockets.add(socket);socket.on('close',()=>sockets.delete(socket));
    record({path:request.url,applicationCookie:(request.headers.cookie||'').includes('zuse_session=')});
    const accept=crypto.createHash('sha1').update(request.headers['sec-websocket-key']+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    socket.on('data',data=>{if((data[0]&15)===8){socket.end();return;}const text=Buffer.from('WebSocket connected');socket.write(Buffer.concat([Buffer.from([0x81,text.length]),text]));});
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  process.send({url:`http://127.0.0.1:${server.address().port}`});
  process.on('disconnect',()=>process.exit(0));
  process.on('message',()=>{server.close(()=>process.exit(0));server.closeAllConnections();for(const socket of sockets)socket.destroy();});
}

if (process.argv[2] === '--serve') await serve();
export async function previewPage() {
  const requests=[];
  const child=fork(fileURLToPath(import.meta.url),['--serve'],{stdio:['ignore','ignore','ignore','ipc']});
  const url=await new Promise((resolve,reject)=>{
    const deadline=setTimeout(()=>{child.kill();reject(new Error('Preview fixture startup deadline'));},10000);
    child.once('error',error=>{clearTimeout(deadline);reject(error);});
    child.on('message',value=>{if(value.url){clearTimeout(deadline);resolve(value.url);}else requests.push(value);});
  });
  return {url,requests,close:()=>new Promise(resolve=>{if(child.exitCode!==null||child.signalCode!==null){resolve();return;}child.once('exit',resolve);child.send('close');})};
}
