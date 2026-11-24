// Measures agent input to decoded viewer pixels on a common clock.
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { readFileSync } from 'node:fs';

const require = createRequire(resolve(process.env.PLAYWRIGHT_NODE_MODULES || 'node_modules', '../monitor.cjs'));
const { chromium } = require('playwright');
const options = { executablePath: process.env.CHROMIUM || '/usr/bin/chromium' };
const tool = (...args) => execFileSync('zuse-monitor', args, { encoding: 'utf8' }).trim();
const guest = await chromium.launch({ ...options, headless: false });
const viewer = await chromium.launch(options);
try {
  const app = await guest.newPage({viewport:null});
  await app.setContent(`<style>body{height:9000px;background:repeating-linear-gradient(white,white 20px,#ccc 21px)}
    #marker{position:fixed;inset:0 auto auto 0;width:96px;height:96px;background:#00ff00}
    input{position:fixed;left:100px;top:10px}</style><div id="marker"></div><input autofocus>
    <script>let red=false; function mark(){red=!red;marker.style.background=red?'#ff0000':'#00ff00'}
    oninput=mark; onwheel=mark;</script>`);
  await app.bringToFront();
  tool('key', 'F11');
  await app.waitForFunction(() => innerHeight === screen.height);
  const page = await viewer.newPage({viewport:{width:1920,height:1080}});
  const port = Number(process.env.MONITOR_VIEWER_PORT || 6901);
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw Error('invalid port');
  const password = tool('password');
  const start = performance.now();
  await page.goto(`http://127.0.0.1:${port}/#${encodeURIComponent(password)}`);
  const visible = red => page.waitForFunction(red => {
    const video = document.querySelector('canvas');
    if (!video || !window.zuseMonitorStats?.codec) return false;
    const ctx = (window.markerCanvas ||= document.createElement('canvas')).getContext('2d', {willReadFrequently:true});
    ctx.drawImage(video, 32, 32, 1, 1, 0, 0, 1, 1);
    const [r,g] = ctx.getImageData(0,0,1,1).data;
    return red ? r > 200 && g < 70 : g > 200 && r < 70;
  }, red, {timeout:10000});
  await visible(false);
  const firstFrameMs = Math.round(performance.now() - start);
  // Isolate CPU of the encoder from the co-located viewer and load generator.
  const pid = execFileSync('pgrep',['-u',String(process.getuid()),'-x','zuse-capture'],{encoding:'utf8'}).trim();
  if (!/^\d+$/.test(pid)) throw Error('expected one streamer for the benchmark');
  const ticks = () => readFileSync('/proc/'+pid+'/stat','utf8').split(' ').slice(13,15).reduce((a,v)=>a+Number(v),0);
  const tickHz = Number(execFileSync('getconf',['CLK_TCK']));
  const results = {};
  let red = false;
  for (const workload of ['type','scroll']) {
    const samples = [];
    const cpuStart = ticks(), windowStart = performance.now();
    for (let i=0;i<20;i++) {
      const t = performance.now();
      if (workload === 'type') tool('type','x');
      else tool('scroll','down');
      red = !red;
      await visible(red);
      samples.push(performance.now()-t);
    }
    const cores = (ticks()-cpuStart)/tickHz*1000/(performance.now()-windowStart);
    samples.sort((a,b)=>a-b);
    results[workload] = {p50Ms:Math.round(samples[9]),p95Ms:Math.round(samples[18]),encoderCores:+cores.toFixed(2)};
  }
  await app.evaluate(() => {
    const sample = window.animationSource = {ticks:0, start:performance.now()};
    function count() {
      sample.ticks++;
      if (!sample.done) requestAnimationFrame(count);
    }
    requestAnimationFrame(count);
    document.body.animate([{ backgroundPosition: '0px 0px' }, { backgroundPosition: '0px 210px' }],
      { duration: 1000, iterations: Infinity });
  });
  const cpuStart = ticks(), windowStart = performance.now();
  const animation = await page.evaluate(() => new Promise(resolve => {
    const gaps = [], bitrate = [];
    let last = performance.now();
    function frame() {
      const now=performance.now();
      gaps.push(now-last);
      bitrate.push(window.zuseMonitorStats.kbps || 0);
      last = now;

    }
    window.addEventListener('monitor-frame',frame);
    setTimeout(() => {
      window.removeEventListener('monitor-frame',frame);
      resolve({frames:gaps.length, stallsOver50ms:gaps.filter(ms => ms>50).length,
        maxGapMs:Math.round(Math.max(...gaps)), meanKbps:Math.round(bitrate.reduce((a,b)=>a+b,0)/bitrate.length)});
    }, 5000);
  }));
  const elapsed = (performance.now()-windowStart)/1000;
  animation.guestRafHz = await app.evaluate(() => {
    const sample = window.animationSource;
    sample.done = true;
    return +(sample.ticks * 1000 / (performance.now() - sample.start)).toFixed(1);
  });
  animation.viewerFps = +(animation.frames/elapsed).toFixed(1);
  animation.encoderCores = +((ticks()-cpuStart)/tickHz/elapsed).toFixed(2);
  console.log(JSON.stringify({firstFrameMs,...results, animation}));
} finally {
  await Promise.all([guest.close(),viewer.close()]);
}
