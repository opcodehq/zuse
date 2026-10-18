// Ephemeral browser WebRTC. No audio, SDP or device state enters browser storage.
let current;
let report;
const message = (scope, kind, value = {}) => {
  if (current === scope && !scope.closed) {
    try { report(JSON.stringify({voiceMedia: {attempt: scope.key, kind, ...value}})); }
    catch { close(scope); }
  }
};
function close(scope) {
  if (!scope || scope.closed) return;
  scope.closed = true;
  clearInterval(scope.meter);
  clearInterval(scope.watchdog);
  for (const cancel of [...scope.waiters]) cancel();
  for (const timer of scope.timers) clearTimeout(timer);
  for (const track of scope.stream?.getTracks() || []) track.stop();
  scope.pc?.close();
  if(scope.motion)scope.motion.removeEventListener('change',scope.motionChanged);
  for (const node of scope.nodes) { try { node.disconnect(); } catch {} }
  if (scope.ctx) void scope.ctx.close().catch(() => {});
  if (current === scope) current = undefined;
}
function fail(scope, error) {
  const text = error?.name === 'NotAllowedError' ? 'Microphone permission was denied. Allow access and start a fresh call.' :
    error?.name === 'NotFoundError' ? 'No microphone is available on this computer.' :
    error?.name === 'NotSupportedError' ? 'This browser does not support voice calls.' :
    'The voice media connection failed. Start a fresh call to try again.';
  message(scope, 'error', {message: text});
  close(scope);
}
function live(scope) {
  if (scope.closed || current !== scope) throw new Error('Retired call');
}
function wait(scope, target, event, ready, milliseconds) {
  return new Promise((resolve, reject) => {
    let timer, finished = false;
    const cancel = () => done(new Error('Retired call'));
    const done = error => { if (finished) return; finished = true; clearTimeout(timer); scope.timers.delete(timer); scope.waiters.delete(cancel); target.removeEventListener(event, check); error ? reject(error) : resolve(); };
    const check = () => { try { live(scope); if (ready()) done(); } catch (error) { done(error); } };
    scope.waiters.add(cancel);
    target.addEventListener(event, check);
    timer = setTimeout(() => done(new Error('Media deadline exceeded')), milliseconds);
    scope.timers.add(timer); check();
  });
}
function analyser(scope, stream) {
  const source = scope.ctx.createMediaStreamSource(stream);
  const node = scope.ctx.createAnalyser(); node.fftSize = 256;
  source.connect(node); scope.nodes.push(source,node);
  return {node, samples: new Uint8Array(node.fftSize)};
}
function peak(meter) {
  if (!meter) return 0;
  meter.node.getByteTimeDomainData(meter.samples);
  let loudness = 0; for (const value of meter.samples) loudness = Math.max(loudness, Math.abs(value - 128) / 128);
  return Math.min(65535, Math.round(loudness * 65535));
}
function applyMute(scope) {
  live(scope);
  for (const track of scope.stream?.getAudioTracks() || []) track.enabled = scope.activated && !scope.muted;
  if (scope.gain) scope.gain.gain.value = scope.activated ? 1 : 0;
}
export function installVoiceMedia(callback) {
  report = callback;
  window.addEventListener('pagehide', () => close(current));
  const unlock = () => { if (current?.ctx?.state === 'suspended') void current.ctx.resume().catch(() => {}); };
  document.addEventListener('pointerdown',unlock,{passive:true});
  document.addEventListener('keydown',unlock,{passive:true});
}
export function voiceMediaCommand(serialized) {
  const command = JSON.parse(serialized);
  if (command.action === 'stop') { if (current?.key === command.attempt) close(current); return; }
  if (command.action === 'prepare') {
    close(current);
    const scope = {key: command.attempt, closed: false, activated: false, muted: false, nodes: [], timers: new Set(), waiters: new Set()};
    current = scope;
    scope.initialDeadline = performance.now() + 150000;
    scope.watchdog = setInterval(() => {
      if (!scope.closed && performance.now() >= (scope.leaseUntil || scope.initialDeadline)) fail(scope,new Error('Voice controller expired'));
    },1000);
    void (async () => {
      if (!window.isSecureContext || !navigator.mediaDevices?.getUserMedia || !window.RTCPeerConnection || !window.AudioContext) throw new DOMException('Voice unsupported','NotSupportedError');
      const stream = await navigator.mediaDevices.getUserMedia({audio:{echoCancellation:true,noiseSuppression:true,autoGainControl:true},video:false});
      // Permission can resolve after cancellation. Retire those tracks immediately.
      for (const track of stream.getTracks()) track.enabled = false;
      if (scope.closed || current !== scope) { for (const track of stream.getTracks()) track.stop(); return; }
      scope.stream = stream;
      scope.ctx = new AudioContext();
      scope.gain = scope.ctx.createGain(); scope.gain.gain.value = 0; scope.gain.connect(scope.ctx.destination); scope.nodes.push(scope.gain);
      scope.microphone = analyser(scope,stream);
      scope.pc = new RTCPeerConnection({bundlePolicy:'max-bundle'});
      scope.pc.createDataChannel('oai-events');
      for (const track of stream.getAudioTracks()) { track.onended = () => fail(scope,new Error('Microphone ended')); scope.pc.addTrack(track,stream); }
      scope.pc.ontrack = event => {
        if (scope.closed || current !== scope) return;
        scope.speaker = analyser(scope,event.streams[0] || new MediaStream([event.track]));
        scope.speaker.node.connect(scope.gain);
      };
      scope.pc.onconnectionstatechange = () => {
        if (scope.closed) return;
        if (scope.pc.connectionState === 'failed' || scope.pc.connectionState === 'closed') fail(scope,new Error('Connection ended'));
        if (scope.pc.connectionState === 'disconnected') {
          const timer=setTimeout(()=>{scope.timers.delete(timer);if(!scope.closed&&scope.pc.connectionState==='disconnected')fail(scope,new Error('Connection lost'));},3000);scope.timers.add(timer);
        }
      };
      const motion=matchMedia('(prefers-reduced-motion: reduce)');
      scope.motion=motion;scope.motionChanged=()=>message(scope,'motion',{reduced:motion.matches});
      motion.addEventListener('change',scope.motionChanged);
      message(scope,'prepared',{reduced:motion.matches});
    })().catch(error => fail(scope,error));
    return;
  }
  const scope = current;
  if (!scope || scope.closed || scope.key !== command.attempt) return;
  if (command.action === 'lease') {
    if (scope.leaseUntil && performance.now() >= scope.leaseUntil) { fail(scope,new Error('Voice lease expired')); return; }
    scope.leaseUntil = performance.now() + 15000; return;
  }
  void (async () => {
    if (command.action === 'mute') { scope.muted = !!command.value.muted; applyMute(scope); return; }
    if (command.action === 'offer') {
      await scope.pc.setLocalDescription(await scope.pc.createOffer()); live(scope);
      await wait(scope,scope.pc,'icegatheringstatechange',()=>scope.pc.iceGatheringState==='complete',20000); live(scope);
      const sdp=scope.pc.localDescription?.sdp;
      if (!sdp || new TextEncoder().encode(sdp).length > 65536) throw new Error('Invalid offer');
      message(scope,'offer',{sdp}); return;
    }
    if (command.action === 'answer') {
      await scope.pc.setRemoteDescription({type:'answer',sdp:command.value.sdp}); live(scope);
      await wait(scope,scope.pc,'connectionstatechange',()=>scope.pc.connectionState==='connected',25000); live(scope);
      message(scope,'ready'); return;
    }
    if (command.action === 'activate') {
      scope.muted = !!command.value.muted;
      await scope.ctx.resume(); live(scope);
      scope.activated = true; applyMute(scope);
      scope.meter = setInterval(()=>{if(!scope.closed)message(scope,'levels',{microphone:scope.muted?0:peak(scope.microphone),speaker:peak(scope.speaker)});},100);
      message(scope,'active');
    }
  })().catch(error => fail(scope,error));
}
