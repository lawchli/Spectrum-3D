// Shared zero-dependency browser runner. Only starts/stops its own disposable Chrome.
import { spawn } from 'node:child_process';
import { access, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { connectCdp } from './cdp.mjs';

export function options(extra = {}) {
  const { values } = parseArgs({ options: {
    port: { type: 'string', default: '8765' }, url: { type: 'string' },
    'cdp-port': { type: 'string' }, chrome: { type: 'string' },
    'target-url': { type: 'string' },
    name: { type: 'string', default: 'tungsten' },
    date: { type: 'string', default: new Date().toLocaleDateString('en-CA', { timeZone: 'Asia/Tokyo' }) },
    help: { type: 'boolean', short: 'h' }, ...extra,
  } });
  values.url ||= `http://127.0.0.1:${values.port}/`;
  return values;
}

export function safeName(value) {
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]*$/.test(value)) throw new Error(`Invalid file name: ${value}`);
  return value;
}

export async function withBrowser(opts, task) {
  let child, dir, api, stderr = '';
  try {
    let port = Number(opts['cdp-port']);
    if (!port) {
      const candidates = [opts.chrome, process.env.CHROME_BIN,
        '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
        '/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser'].filter(Boolean);
      let executable;
      for (const path of candidates) { try { await access(path); executable = path; break; } catch {} }
      if (!executable) throw new Error('Chrome not found; pass --chrome /path/to/chrome or --cdp-port PORT');
      dir = await mkdtemp(join(tmpdir(), 'spectrum-chrome-'));
      child = spawn(executable, ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${dir}`,
        '--autoplay-policy=no-user-gesture-required', '--hide-scrollbars', '--no-first-run',
        // A fake microphone, granted without a prompt, for ui-check's input device checks.
        '--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream',
        '--no-default-browser-check', 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] });
      child.stderr.on('data', data => { stderr = (stderr + data).slice(-6000); });
      let spawnError;
      child.on('error', err => { spawnError = err; });
      const deadline = Date.now() + 20000;
      while (!port && Date.now() < deadline) {
        if (spawnError) throw spawnError;
        if (child.exitCode !== null || child.signalCode) throw new Error(`Chrome exited: ${stderr}`);
        try { port = Number((await readFile(join(dir, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); } catch {}
        if (!port) await new Promise(r => setTimeout(r, 100));
      }
      if (!port) throw new Error(`Chrome did not start: ${stderr}`);
    }
    // Android may not support /json/new: attach only to the exact, freshly opened test URL.
    api = await connectCdp({ port, newTab: !opts['target-url'], targetUrl: opts['target-url'], closeTarget: true });
    return await task(api);
  } catch (error) {
    if (api?.logs.length) console.error(api.logs.join('\n'));
    throw error;
  } finally {
    if (api) await api.close().catch(() => {});
    if (child && child.exitCode === null && !child.signalCode) {
      await new Promise(resolve => {
        const timer = setTimeout(() => { child.kill('SIGKILL'); }, 3000);
        child.once('exit', () => { clearTimeout(timer); resolve(); });
        child.kill('SIGTERM');
      });
    }
    if (dir) await rm(dir, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 });
  }
}

// Per-tab settings fixture, without touching the user's stored preferences (also with --cdp-port).
export async function openLab(a, url, { width = 1440, height = 900, mobile = false, nativeViewport = false, ui = {} } = {}) {
  if (!nativeViewport) await a.viewport(width, height, { mobile });
  await a.send('Page.addScriptToEvaluateOnNewDocument', { source: `
    if (location.origin === ${JSON.stringify(new URL(url).origin)}) {
      const key = 'spectrum-visualizer.settings.v1';
      const nativeGet = Storage.prototype.getItem;
      Storage.prototype.getItem = function(k) {
        return this === localStorage && k === key
          ? ${JSON.stringify(JSON.stringify({ ui: { lang: 'zh', ...ui } }))}
          : nativeGet.call(this, k);
      };
      const nativeSet = Storage.prototype.setItem;
      Storage.prototype.setItem = function(k, v) {
        if (this !== localStorage || k !== key) nativeSet.call(this, k, v);
      };
    }` });
  await a.goto(url);
  await a.waitFor('!!window.spectrumLab', nativeViewport ? 45000 : 15000);
}

export async function testAudio(a) {
  await a.eval(`document.getElementById('srcSynth').click()`);
  await a.waitFor('spectrumLab.state.test === 1');
  await a.eval(`document.getElementById('srcSynth').click()`);
  await a.waitFor('spectrumLab.state.test === 2 && spectrumLab.state.rowsPushed > 0');
  const before = await a.eval('spectrumLab.state.rowsPushed');
  await a.waitFor(`spectrumLab.state.rowsPushed > ${before}`, 5000);
}

export async function configure(a, config = {}) {
  await a.eval(`(() => {
    const c = ${JSON.stringify(config)}, lab = spectrumLab;
    const change = (id, value, event = 'change') => {
      const el = document.getElementById(id);
      if (el.type === 'checkbox') el.checked = value; else el.value = value;
      el.dispatchEvent(new Event(event, { bubbles: true }));
    };
    if (c.rowRate) document.querySelector('#rowRate [data-value="' + c.rowRate + '"]').click();
    if (c.sub) document.querySelector('#bandSub [data-value="' + c.sub + '"]').click();
    if (c.seconds) change('seconds', c.seconds, 'input');
    if (c.style) change('style', c.style);
    if (c.frame) change('frame', c.frame);
    for (const [key, id] of [['peaks', 'optPeaks'], ['maxHold', 'optMaxHold'], ['piano', 'optPiano'], ['lowLevel', 'optLowLevel']]) {
      if (key in c) change(id, c[key]);
    }
    for (const key of ['spectrum', 'waterfall']) {
      if (key in c && lab.settings.layers[key] !== c[key]) document.getElementById(key === 'spectrum' ? 'layerSpectrum' : 'layerWaterfall').click();
    }
    if (c.view) lab.setView(c.view);
  })()`);
}
