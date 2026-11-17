// 用无头 Chrome 截图验收网页版（2026-11-04）。零依赖，只用 Node 22 自带的 fetch 和 WebSocket。
//
// 为什么不用 Playwright 插件或内置浏览器：Playwright 的无头浏览器没有音频输出，AudioContext 的
// 时钟停在 0，地形是空的；内置浏览器面板不在前台时被限速，截不了整页文件。完整版 Chrome 的新
// 无头模式有音频，GPU 用的是 M2（ANGLE Metal），帧率和真实浏览器一致。
//
// 用法：
//   1. 起预览服务器（.claude/launch.json 的 web，端口 8765）。
//   2. 后台起 Chrome（profile 放 scratchpad，不要放仓库里）：
//      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new \
//        --remote-debugging-port=9333 --user-data-dir=<scratchpad>/chrome-profile \
//        --autoplay-policy=no-user-gesture-required --hide-scrollbars --no-first-run about:blank
//   3. node scripts/cdp.mjs <场景>.mjs。场景文件 default export 一个 async (a) => {}，
//      a 有 viewport、goto、eval、click(选择器)、hover、move、key、wait、shot(路径, 裁剪)。
//      例子：
//        export default async (a) => {
//          await a.viewport(1440, 900); await a.goto('http://127.0.0.1:8765/');
//          await a.waitFor('!!window.spectrumLab');
//          await a.click('#srcSynth'); await a.wait(300); await a.click('#srcSynth');  // 测试音频 2
//          await a.wait(4000); await a.shot('previews/2026-11-04-x.jpg');
//        };
//   控制台错误和异常在最后打印。用完 kill 掉 Chrome。
import { writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export async function connectCdp({ port = 9333, newTab = false, targetUrl, closeTarget = newTab } = {}) {
const base = `http://127.0.0.1:${port}`;
let target;
if (newTab) {
  const response = await fetch(`${base}/json/new?about:blank`, { method: 'PUT' });
  if (!response.ok) throw new Error(`Cannot create test tab (${response.status}); on Android, open a unique test URL and pass --target-url`);
  target = await response.json();
} else {
  target = (await (await fetch(`${base}/json/list`)).json()).find(t => t.type === 'page' && (!targetUrl || t.url === targetUrl));
}
if (!target) throw new Error(`No matching test page at ${base}${targetUrl ? ` for ${targetUrl}` : ''}`);
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.addEventListener('open', resolve, { once: true });
  ws.addEventListener('error', reject, { once: true });
});
let id = 0; const pending = new Map(); const logs = [];
ws.addEventListener('message', ev => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { const { res, rej } = pending.get(m.id); pending.delete(m.id); m.error ? rej(new Error(JSON.stringify(m.error))) : res(m.result); }
  else if (m.method === 'Runtime.exceptionThrown') logs.push('EXC ' + (m.params.exceptionDetails.exception?.description || JSON.stringify(m.params.exceptionDetails)).slice(0, 4000));
  else if (m.method === 'Runtime.consoleAPICalled' && ['error', 'warning'].includes(m.params.type)) logs.push(m.params.type + ' ' + m.params.args.map(a => a.value ?? a.description).join(' '));
  else if (m.method === 'Log.entryAdded' && m.params.entry.level !== 'verbose') {
    const e = m.params.entry;
    // Existing app has no favicon (plan D3). Do not fail A/E for that known 404.
    if (!(e.url?.endsWith('/favicon.ico') && /404/.test(e.text))) logs.push('log ' + e.level + ' ' + e.text + ' ' + (e.url || ''));
  }
});
ws.addEventListener('close', () => {
  for (const { rej, timeout } of pending.values()) { clearTimeout(timeout); rej(new Error('CDP disconnected')); }
  pending.clear();
});
const send = (method, params = {}, timeoutMs = 30000) => new Promise((resolve, reject) => {
  const i = ++id;
  const timeout = setTimeout(() => { pending.delete(i); reject(new Error(`CDP timeout: ${method}`)); }, timeoutMs);
  pending.set(i, { res: value => { clearTimeout(timeout); resolve(value); }, rej: err => { clearTimeout(timeout); reject(err); }, timeout });
  ws.send(JSON.stringify({ id: i, method, params }));
});
await send('Runtime.enable'); await send('Log.enable'); await send('Page.enable');
await send('Page.bringToFront');

const api = {
  send, logs,
  wait: ms => new Promise(r => setTimeout(r, ms)),
  async viewport(w, h, { dpr = 1, mobile = false } = {}) { await send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: dpr, mobile }); },
  async goto(url) {
    const r = await send('Page.navigate', { url });
    if (r.errorText) throw new Error(r.errorText);
    await api.waitFor('document.readyState === "complete"');
  },
  async waitFor(expr, timeoutMs = 15000) {
    const deadline = Date.now() + timeoutMs;
    do {
      try { if (await api.eval(expr)) return; } catch (e) {
        if (!/context|navigat/i.test(e.message)) throw e;
      }
      await api.wait(100);
    } while (Date.now() < deadline);
    throw new Error(`Timed out waiting for ${expr}`);
  },
  async eval(expr, timeoutMs = 30000) {
    const r = await send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true, userGesture: true }, timeoutMs);
    if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 500));
    return r.result.value;
  },
  async center(sel) {
    const b = await api.eval(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null; const r = e.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()`);
    if (!b) throw new Error('no element ' + sel); return b;
  },
  async move(x, y) { await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y }); },
  async click(sel) {
    const [x, y] = await api.center(sel);
    await api.move(x, y);
    await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
    await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
    await api.wait(120);
  },
  async hover(sel) { const [x, y] = await api.center(sel); await api.move(x, y); await api.wait(250); },
  async touch(enabled = true) { await send('Emulation.setTouchEmulationEnabled', { enabled, maxTouchPoints: 2 }); },
  async tap(sel) {
    const [x, y] = await api.center(sel);
    await send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y, id: 1 }] });
    await send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    await api.wait(150);
  },
  async key(key, code, opts = {}) {
    const text = key.length === 1 ? key : undefined;
    const vk = opts.vk;
    await send('Input.dispatchKeyEvent', { type: text ? 'keyDown' : 'rawKeyDown', key, code: code || key, text, windowsVirtualKeyCode: vk, modifiers: opts.mod || 0 });
    await send('Input.dispatchKeyEvent', { type: 'keyUp', key, code: code || key, windowsVirtualKeyCode: vk, modifiers: opts.mod || 0 });
    await api.wait(80);
  },
  async shot(path, clip) {
    const r = await send('Page.captureScreenshot', { format: 'jpeg', quality: 88, ...(clip ? { clip: { ...clip, scale: 1 } } : {}) });
    writeFileSync(path, Buffer.from(r.data, 'base64'));
  },
  async close() {
    ws.close();
    if (closeTarget) await fetch(`${base}/json/close/${target.id}`);
  },
};
return api;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const api = await connectCdp({ port: Number(process.env.CDP_PORT || 9333) });
  try {
    const scenario = (await import(pathToFileURL(process.argv[2]).href)).default;
    const out = await scenario(api);
    if (out !== undefined) console.log(typeof out === 'string' ? out : JSON.stringify(out, null, 1));
    if (api.logs.length) throw new Error(api.logs.join('\n'));
  } catch (e) { console.error(e); process.exitCode = 1; }
  finally { await api.close(); }
}
