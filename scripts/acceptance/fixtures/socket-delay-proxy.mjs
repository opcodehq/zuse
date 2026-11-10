// Adds propagation delay to both directions of an actual WebSocket connection.
// DevTools' HTTP latency setting alone is not a packet-level RTT simulation.
import http from 'node:http';
import net from 'node:net';

export async function socketDelayProxy(target, roundTripMs) {
  const upstream = new URL(target);
  const connections = new Set();
  const timers = new Set();
  const traffic=[];
  let stage='startup';
  let compressed=false;
  let network;
  const next={download:0,upload:0};
  const track = socket => {
    connections.add(socket);
    socket.on('close', () => connections.delete(socket));
    socket.on('error', () => socket.destroy());
    return socket;
  };
  const later = work => {
    const timer = setTimeout(() => {timers.delete(timer);work();}, roundTripMs / 2);
    timers.add(timer);
  };
  const schedule=(work,direction,size,delay)=>{
    const now=Date.now();
    const rate=network?.[direction==='download'?'downloadBytesPerSecond':'uploadBytesPerSecond'];
    const due=rate>0?Math.max(now+delay,next[direction])+size*1000/rate:now+delay;
    if(rate>0)next[direction]=due;
    const timer=setTimeout(()=>{timers.delete(timer);work();},Math.max(0,due-now));
    timers.add(timer);
  };
  const forward=(from,to,direction,delay,observe=false)=>{
    let queued=0,waitingDrain=false;
    let tail=Promise.resolve();
    const resume=()=>{if(queued<128*1024&&!waitingDrain&&!from.destroyed)from.resume();};
    from.on('data',data=>{
      const phase=stage;queued+=data.length;
      if(queued>256*1024)from.pause();
      if(observe&&direction==='download'&&data.includes(Buffer.from('HTTP/1.1 101')))
        compressed ||= data.toString('latin1').toLowerCase().includes('permessage-deflate');
      const ready=new Promise(resolve=>schedule(resolve,direction,data.length,typeof delay==='function'?delay():delay));
      // Independent millisecond timers can wake out of order when tiny
      // frames share a deadline. TCP delivery must remain strictly FIFO,
      // especially when native deflate retains a cross-frame dictionary.
      tail=tail.then(()=>ready).then(()=>{
        queued-=data.length;
        if(!to.destroyed){
          if(observe)traffic.push({stage:phase,direction,bytes:data.length,at:Date.now()});
          if(!to.write(data)){waitingDrain=true;from.pause();to.once('drain',()=>{waitingDrain=false;resume();});}
        }
        resume();
      }).catch(()=>to.destroy());
    });
    from.on('end',()=>{
      const ready=new Promise(resolve=>schedule(resolve,direction,0,typeof delay==='function'?delay():delay));
      tail=tail.then(()=>ready).then(()=>{if(!to.destroyed)to.end();}).catch(()=>to.destroy());
    });
    from.on('error',()=>to.destroy());
  };
  const server = http.createServer((request, response) => {
    const earliestResponse=Date.now()+(network?.latencyMs??0);
    const forwarded = http.request({hostname:upstream.hostname, port:upstream.port,
      method:request.method, path:request.url, headers:request.headers}, reply => {
      response.writeHead(reply.statusCode, reply.headers);
      forward(reply,response,'download',()=>Math.max(0,earliestResponse-Date.now()));
    });
    forwarded.on('error', () => {
      if(response.destroyed||response.writableEnded)return;
      if(response.headersSent){response.destroy();return;}
      response.writeHead(502);response.end();
    });
    forward(request,forwarded,'upload',0);
  });
  server.on('connection', track);
  server.on('upgrade', (request, socket, head) => {
    const peer = track(net.connect(Number(upstream.port), upstream.hostname));
    socket.on('error', () => peer.destroy());
    peer.on('error', () => socket.destroy());
    // Limits apply to encoded TCP bytes, before WebSocket decompression.
    // Aggregate both HTTP and WS traffic in each direction. Queueing has
    // bounded backpressure rather than retaining unlimited delayed chunks.
    forward(socket,peer,'upload',roundTripMs/2,true);
    forward(peer,socket,'download',roundTripMs/2,true);
    socket.on('close',()=>later(()=>peer.destroy()));
    peer.on('close',()=>later(()=>socket.destroy()));
    peer.once('connect', () => {
      const headers = request.rawHeaders.reduce((lines, value, index, all) =>
        index % 2 ? lines : lines + value + ': ' + all[index + 1] + '\r\n', '');
      later(() => {
        if(peer.destroyed) return;
        peer.write(`${request.method} ${request.url} HTTP/${request.httpVersion}\r\n${headers}\r\n`);
        if(head.length) peer.write(head);
      });
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = new URL(target);
  browser.port = String(server.address().port);
  return {
    url:browser.href,
    traffic,
    setStage:value=>{stage=value;},
    setNetworkProfile:value=>{network=value;next.download=0;next.upload=0;},
    get webSocketDeflate(){return compressed;},
    close:async () => {
      for(const timer of timers) clearTimeout(timer);
      for(const socket of connections) socket.destroy();
      await new Promise(resolve => server.close(resolve));
    },
  };
}
