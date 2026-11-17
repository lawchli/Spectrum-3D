import { STYLES, loadStyle } from './styles.js';
import { RenderScale } from './render-scale.js';
import { LOOK } from './look.js';
import { AudioEngine, ROW_RATES, listInputs, findInput } from './audio.js';
import { BANDS, SUBDIVISIONS, bandCount, bandHz, noteName, dbToByte, uToBand } from './bands.js';
import { OrbitCamera } from './camera.js';
import { Renderer, HISTORY_ROWS, LEVELS, viewMetrics } from './renderer.js';
import { Labels, frontFacing } from './labels.js';
import { PeakTracker, PeakLabels } from './peaks.js';
import { Monitor } from './monitor.js';
import { loadSettings, saveSettings, DEFAULT_SUB } from './settings.js';
import { t, setLang } from './i18n.js';
import { GamepadControl } from './gamepad.js';
import { transform } from './mat4.js';

const $ = (id) => document.getElementById(id);
const stage = $('stage');
const canvas = $('view');
// No MSAA on Android. On the XZ1 Compact (Adreno 540) a multisampled canvas the size of the
// landscape screen makes the GPU fault ("kgsl gpu fault ... 3 faults in 3000 msecs") and Chrome
// loses the context, the same driver fault that kills the native app at 4x (its default is
// now 0x too). Desktops keep it.
const ANDROID = /Android/i.test(navigator.userAgent);
const gl = canvas.getContext('webgl2', {
  antialias: !ANDROID, alpha: false, desynchronized: true, powerPreference: 'high-performance',
});
if (!gl) throw new Error('WebGL2 not available');

const audio = new AudioEngine();
const camera = new OrbitCamera(LOOK);
const labels = new Labels($('labels'), LOOK);
let renderer = await Renderer.create(gl, LOOK);
const settings = loadSettings();
// Phone-sized: narrower than 600, or a landscape phone (the XZ1 Compact's Chrome is 640 x 276).
const compactViewport = matchMedia('(max-width: 600px), (max-width: 900px) and (max-height: 500px)');
function hudCollapsed() {
  return settings.ui.hud === 'collapsed' || (settings.ui.hud === 'auto' && compactViewport.matches);
}
// Collapsed on a phone-sized viewport, the monitor gives up its title bar as well, so the
// spectrum gets the whole height; it opens again from a button at the foot of the panel rail.
function syncHud() {
  const collapsed = hudCollapsed();
  $('hud').hidden = collapsed || !settings.monitor.visible;
  $('hudDock').hidden = !settings.monitor.visible || (collapsed && compactViewport.matches);
  $('hudRail').hidden = !settings.monitor.visible || !collapsed || !compactViewport.matches;
}
{
  // Restore the initial renderer too, before a remembered non-Tungsten style loads.
  const saved = settings.styleOptions.tungsten;
  if (saved && Object.hasOwn(LOOK.styles, saved.style)) LOOK.style = saved.style;
}
const renderScale = new RenderScale(settings.render.scale);
if (!ROW_RATES.includes(settings.analysis.rowRate)) settings.analysis.rowRate = ROW_RATES[0];
if (!renderer.subdivisions.includes(settings.analysis.sub)) settings.analysis.sub = 1;
const defaultSub = renderer.subdivisions.includes(DEFAULT_SUB) ? DEFAULT_SUB : 1;
const peaks = new PeakTracker();
const peakLabels = new PeakLabels($('peaks'));
const monitor = new Monitor(gl, $('hud'));
const gamepad = new GamepadControl();

// Live display smoothing, same constants as LiveAnalysis.kt on the phone.
const RELEASE_DB_PER_S = 36;
const PEAK_HOLD_S = 1.5;
const PEAK_FALL_DB_PER_S = 18;
let BASE_HEIGHT = LOOK.size.height;
// Low levels: the floor drops from the look's (-96 dB) to the byte format's -120 dB.
let BASE_FLOOR = LOOK.db.floor;
const LOW_FLOOR = -120;
const PHONE_ROW_RATE = 50;  // what the Spar recorder backend sends today, one band per semitone
const TILT_DB_PER_OCTAVE = 4.5;  // the front view's optional tilt, as on FabFilter Pro-Q's analyser

let currentStyle = 'tungsten';
let switchingStyle = false;
const state = {
  running: false,
  frozen: false,
  paused: false,  // stops the display loop so tick() can be stepped by hand
  flat: false,    // spectrum without the waterfall: held in the front view
  viewBeforeFlat: null,
  hoverBand: null,
  status: { key: 'notStarted' },  // or { text } for a file name
  test: 0,        // which test sound plays: 0 none, 1 or 2
  labelled: [],   // peaks labelled last frame, lit on the piano strip
  phoneMode: false,
  rowRate: 60,                 // data rows a second (settings.analysis.rowRate, or 50 like the phone)
  sub: 1,                      // data bands per semitone
  seconds: 4,                  // time across the framed part; the tail runs on LOOK.size.tail times as long
  cols: 2 * (BANDS - 1) + 1,   // mesh columns
  keyLit: new Float32Array(BANDS),
  phase: 0,
  rowsPushed: 0,
  mel: 0,                      // frequency axis, gliding between 0 (semitone) and 1 (mel)
};

// Per data band: latest analysis, displayed (with release), peak hold, max hold, bytes.
function allocBands(sub) {
  const n = bandCount(sub);
  Object.assign(state, {
    raw: new Float32Array(n).fill(-120),
    live: new Float32Array(n).fill(-120),   // raw through envelope(), what the front spectrum follows
    scratch: new Float32Array(n),
    shown: new Float32Array(n).fill(-120),
    peak: new Float32Array(n).fill(-120),
    peakAge: new Float32Array(n),
    maxHold: new Float32Array(n).fill(-120),
    bytes: new Uint8Array(n),
  });
}
allocBands(1);

// Data rows across the framed part, and down the whole length including the tail. Far
// down the tail the mesh reads the coarsest level, whose ring must still hold the end.
function nearRows() { return Math.round(state.seconds * state.rowRate); }
function spanRows() { return Math.min((HISTORY_ROWS - 2) * 2 ** (LEVELS - 1), Math.round(nearRows() * LOOK.size.tail)); }

// ---- Floating ? tooltip (positioned outside #panel so it is never clipped) ----
const tooltipEl = $('tooltip');
let activeHelp = null;
function positionTooltip(btn) {
  const tip = btn.dataset.tip;
  if (!tip) return;
  tooltipEl.textContent = tip;
  tooltipEl.hidden = false;
  const panelRect = $('panel').getBoundingClientRect();
  const btnRect = btn.getBoundingClientRect();
  const tipRect = tooltipEl.getBoundingClientRect();
  const left = Math.min(window.innerWidth - tipRect.width - 8, Math.max(8, panelRect.right + 8));
  const top = Math.min(window.innerHeight - tipRect.height - 8, Math.max(8, btnRect.top - 4));
  tooltipEl.style.left = `${left.toFixed(0)}px`;
  tooltipEl.style.top = `${top.toFixed(0)}px`;
}
function openHelp(btn) {
  if (activeHelp && activeHelp !== btn) activeHelp.classList.remove('is-open');
  activeHelp = btn;
  btn.classList.add('is-open');
  positionTooltip(btn);
}
function closeHelp(btn) {
  if (btn && activeHelp !== btn) return;
  if (activeHelp) activeHelp.classList.remove('is-open');
  activeHelp = null;
  tooltipEl.hidden = true;
}
for (const btn of document.querySelectorAll('.help[data-i18n-tip]')) {
  btn.addEventListener('pointerenter', () => openHelp(btn));
  btn.addEventListener('pointerleave', () => closeHelp(btn));
  btn.addEventListener('focus', () => openHelp(btn));
  btn.addEventListener('blur', () => closeHelp(btn));
}

// Seconds in view next to the slider; the total length with the tail goes in the ? tip.
function showSeconds() {
  const vars = { s: state.seconds, total: (spanRows() / state.rowRate).toFixed(0) };
  $('secondsValue').textContent = t('secondsValue', vars);
  const slider = $('seconds');
  const pct = ((state.seconds - Number(slider.min)) / (Number(slider.max) - Number(slider.min))) * 100;
  slider.style.setProperty('--pct', `${pct.toFixed(1)}%`);
  const tip = document.querySelector('[data-i18n-tip="timeHint"]');
  tip.dataset.tip = t('timeHint', vars);
  tip.setAttribute('aria-label', tip.dataset.tip);
  if (activeHelp === tip) positionTooltip(tip);
}

function rebuild() {
  // Two mesh columns a semitone (one on each band centre, one between), or one per data band when finer.
  state.cols = (BANDS - 1) * Math.max(2, state.sub) + 1;
  renderer.buildMesh(state.cols, nearRows(), spanRows());
  renderer.melAxis = state.mel;
  renderer.buildGuides(state.seconds);
  labels.build(state.seconds, state.mel);
  showSeconds();
}

// ---- Canvas size, phone frames and engineering drawer badge ----
const FRAMES = { full: null, landscape: [640, 360], portrait: [360, 640], inline: [360, 200] };
let cssW = 1, cssH = 1;
let currentFrame = 'full';
let currentFft = 'multi';

// Whatever in the drawer differs from the defaults, shown on its title while it is closed.
function syncAdvBadge() {
  const badge = $('advBadge');
  const f = FRAMES[currentFrame];
  const a = settings.analysis;
  const parts = [
    f && `${f[0]}×${f[1]}`,
    state.phoneMode ? `${PHONE_ROW_RATE}/s` : a.rowRate !== ROW_RATES[0] && `${a.rowRate}/s`,
    !state.phoneMode && a.sub !== defaultSub && `×${a.sub}`,
    a.lowLevel && `−${-LOW_FLOOR}\u202fdB`,
    currentFft !== 'multi' && currentFft,
  ].filter(Boolean);
  badge.hidden = !parts.length;
  badge.textContent = parts.slice(0, 2).join(' ') + (parts.length > 2 ? ` +${parts.length - 2}` : '');
}

function setFrame(name) {
  currentFrame = name;
  const f = FRAMES[name];
  stage.classList.toggle('framed', !!f);
  document.body.classList.toggle('framed-stage', !!f);
  fitFrame();
  const meta = $('frameMeta');
  meta.hidden = !f;
  meta.textContent = f ? `${f[0]} × ${f[1]}\u202fpx` : '';
  syncAdvBadge();
}
function fitFrame() {
  const f = FRAMES[currentFrame];
  const bounds = $('world').getBoundingClientRect();
  const scale = f ? Math.min(1, Math.max(1, bounds.width - 8) / f[0], Math.max(1, bounds.height - 8) / f[1]) : 1;
  stage.style.width = f ? `${f[0] * scale}px` : '';
  stage.style.height = f ? `${f[1] * scale}px` : '';
}
new ResizeObserver(fitFrame).observe($('world'));
function resizeCanvas() {
  const r = stage.getBoundingClientRect();
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  cssW = r.width; cssH = r.height;
  canvas.width = Math.max(1, Math.round(r.width * dpr * renderScale.scale));
  canvas.height = Math.max(1, Math.round(r.height * dpr * renderScale.scale));
}
new ResizeObserver(resizeCanvas).observe(stage);
window.addEventListener('resize', resizeCanvas);

// ---- Controls ----
function bindSelect(id, onChange) { $(id).addEventListener('change', (e) => onChange(e.target.value)); }

for (const name of Object.keys(LOOK.styles)) {
  $('style').append(new Option(LOOK.styles[name]?.label || name, name));
}
$('style').value = LOOK.style;
bindSelect('style', (v) => { LOOK.style = v; rememberStyle(); });
for (const [name, item] of Object.entries(STYLES)) $('visualStyle').append(new Option(item.label, name));
bindSelect('visualStyle', (v) => switchStyle(v).catch(error => { $('visualStyle').value = currentStyle; state.status = { text: error.message }; showStatus(); }));
bindSelect('frame', setFrame);
$('renderScale').value = renderScale.mode;
bindSelect('renderScale', (v) => {
  renderScale.setMode(v); settings.render.scale = renderScale.mode; saveSettings(settings); resizeCanvas();
});
bindSelect('fft', (v) => {
  currentFft = v;
  audio.setResolution(v === 'multi', Number(v) || 8192);
  syncAdvBadge();
});
$('seconds').addEventListener('input', (e) => {
  state.seconds = Number(e.target.value);
  rebuild();
});
$('phone').addEventListener('change', (e) => {
  state.phoneMode = e.target.checked;
  applyAnalysis();
});

// ---- Rows a second, bands per semitone, low levels (remembered in this browser) ----
// Either change starts the history over: rows of another rate or width cannot be converted.
function applyAnalysis() {
  const a = settings.analysis;
  const rate = state.phoneMode ? PHONE_ROW_RATE : a.rowRate;
  const sub = state.phoneMode ? 1 : a.sub;
  if (rate !== state.rowRate || sub !== state.sub) {
    state.rowRate = rate;
    state.sub = sub;
    audio.setBands(sub);
    audio.setRowRate(rate);
    renderer.setBands(sub);
    allocBands(sub);
    peaks.clear();
    rebuild();
  }
  syncAnalysis();
  syncAdvBadge();
}

function syncAnalysis() {
  const a = settings.analysis;
  for (const b of $('rowRate').children) {
    b.classList.toggle('on', Number(b.dataset.value) === a.rowRate);
    b.disabled = state.phoneMode;
  }
  for (const b of $('bandSub').children) {
    b.classList.toggle('on', Number(b.dataset.value) === a.sub);
    b.disabled = state.phoneMode || !renderer.subdivisions.includes(Number(b.dataset.value));
  }
  $('optLowLevel').checked = a.lowLevel;
}

function bindTiers(id, values, key) {
  const seg = $(id);
  for (const v of values) {
    const b = document.createElement('button');
    b.type = 'button';
    b.dataset.value = v;
    b.textContent = v;
    b.addEventListener('click', () => {
      settings.analysis[key] = v;
      saveSettings(settings);
      applyAnalysis();
    });
    seg.append(b);
  }
}
// The panel scrolls in short windows: bring the opened drawer into view.
$('advDrawer').addEventListener('toggle', (e) => {
  if (e.target.open) e.target.scrollIntoView({ block: 'nearest' });
});
bindTiers('rowRate', ROW_RATES, 'rowRate');
bindTiers('bandSub', SUBDIVISIONS, 'sub');
$('optLowLevel').addEventListener('change', (e) => {
  settings.analysis.lowLevel = e.target.checked;
  saveSettings(settings);
  syncAdvBadge();
});
$('monitor').addEventListener('change', (e) => audio.setMonitor(e.target.checked));

// ---- Layers, spectrum options and the monitor (remembered in this browser) ----
function setLayer(name, on) {
  const { layers } = settings;
  layers[name] = on;
  if (!layers.spectrum && !layers.waterfall) layers[name === 'spectrum' ? 'waterfall' : 'spectrum'] = true;
  applyLayers();
  saveSettings(settings);
}

// Spectrum alone turns the camera to the front and holds it there; the waterfall coming
// back returns to the view from before.
function applyLayers() {
  const flat = settings.layers.spectrum && !settings.layers.waterfall;
  if (flat && !state.flat) {
    state.viewBeforeFlat = camera.snapshot();
    camera.setView('front');
  } else if (!flat && state.flat && state.viewBeforeFlat) {
    camera.restore(state.viewBeforeFlat);
  }
  state.flat = flat;
  syncPanel();
}

function clearMaxHold() { state.maxHold.fill(-120); }

function syncPanel() {
  $('layerSpectrum').classList.toggle('on', settings.layers.spectrum);
  $('layerWaterfall').classList.toggle('on', settings.layers.waterfall);
  for (const b of $('axisScale').children) b.classList.toggle('on', b.dataset.value === settings.axis.scale);
  viewButtons.top.disabled = viewButtons.perspective.disabled = state.flat;
  $('optPeaks').checked = settings.spectrum.peaks;
  $('optMaxHold').checked = settings.spectrum.maxHold;
  $('optPiano').checked = settings.spectrum.piano;
  $('optTilt').checked = settings.spectrum.tilt;
  $('monVisible').checked = settings.monitor.visible;
  $('monChips').classList.toggle('is-off', !settings.monitor.visible);
  for (const el of document.querySelectorAll('[data-mon]')) el.checked = settings.monitor[el.dataset.mon];
  syncHud();
  $('btnFreeze').classList.toggle('on', state.frozen);
}

// ---- Language ----
let shownPad = null;
function showStatus() {
  const s = state.status;
  $('status').textContent = s.text ?? t(s.key, s.vars);
}
function applyLanguage(lang) {
  settings.ui.lang = lang;
  setLang(lang);
  syncLayout();
  for (const b of document.querySelectorAll('[data-lang]')) b.classList.toggle('on', b.dataset.lang === lang);
  showStatus();
  showTestButton();
  monitor.relabel();
  shownPad = undefined;  // redraw the gamepad line in the new language
  showSeconds();
  if (activeHelp) positionTooltip(activeHelp);
}
for (const b of document.querySelectorAll('[data-lang]')) {
  b.addEventListener('click', () => { applyLanguage(b.dataset.lang); saveSettings(settings); });
}

for (const b of $('axisScale').children) {
  b.addEventListener('click', () => { settings.axis.scale = b.dataset.value; syncPanel(); saveSettings(settings); });
}
$('layerSpectrum').addEventListener('click', () => setLayer('spectrum', !settings.layers.spectrum));
$('layerWaterfall').addEventListener('click', () => setLayer('waterfall', !settings.layers.waterfall));
for (const [id, key] of [['optPeaks', 'peaks'], ['optMaxHold', 'maxHold'], ['optPiano', 'piano'], ['optTilt', 'tilt']]) {
  $(id).addEventListener('change', (e) => {
    settings.spectrum[key] = e.target.checked;
    if (key === 'maxHold') clearMaxHold();
    saveSettings(settings);
  });
}
$('clearMax').addEventListener('click', clearMaxHold);
function setMonitorVisible(on) {
  if (on && !settings.monitor.visible) monitor.clearMemory();
  settings.monitor.visible = on;
  syncPanel();
  saveSettings(settings);
}
$('monVisible').addEventListener('change', (e) => setMonitorVisible(e.target.checked));
for (const el of document.querySelectorAll('[data-mon]')) {
  el.addEventListener('change', () => { settings.monitor[el.dataset.mon] = el.checked; saveSettings(settings); });
}

// Dock states are independent of M's monitor visibility and H's full-screen mode.
// On a phone-sized viewport (compactViewport) the panel opens over the picture instead of
// beside it, and that is not remembered: the next visit starts collapsed again. Elsewhere an
// explicit open or collapse is remembered. The monitor's 'auto' collapses it there too.
let panelOverlayOpen = false;
function panelCollapsed() {
  return compactViewport.matches ? !panelOverlayOpen : settings.ui.panel === 'collapsed';
}
function syncLayout() {
  const collapsed = panelCollapsed();
  const compact = compactViewport.matches;
  document.body.classList.toggle('compact', compact);
  document.body.classList.toggle('panel-collapsed', collapsed);
  document.body.classList.toggle('panel-overlay', compact && !collapsed);
  document.body.classList.toggle('hud-collapsed', hudCollapsed());
  document.body.classList.toggle('bare', settings.ui.bare);
  $('panel').hidden = collapsed;
  for (const [id, closed, key] of [['panelToggle', collapsed, 'Panel'], ['hudToggle', hudCollapsed(), 'Hud'], ['hudRail', hudCollapsed(), 'Hud']]) {
    const button = $(id);
    button.setAttribute('aria-expanded', String(!closed));
    button.dataset.i18nLabel = `${closed ? 'expand' : 'collapse'}${key}`;
    button.setAttribute('aria-label', t(button.dataset.i18nLabel));
    button.title = t(button.dataset.i18nLabel);
  }
  syncHud();
  fitFrame();
}
compactViewport.addEventListener('change', () => { panelOverlayOpen = false; syncLayout(); });
function setPanelOpen(open) {
  closeHelp();
  if (compactViewport.matches) panelOverlayOpen = open;
  else { settings.ui.panel = open ? 'open' : 'collapsed'; saveSettings(settings); }
  syncLayout();
}
$('panelToggle').addEventListener('click', () => setPanelOpen(document.body.classList.contains('panel-collapsed')));
function toggleHud() {
  settings.ui.hud = hudCollapsed() ? 'open' : 'collapsed';
  syncLayout();
  saveSettings(settings);
}
$('hudToggle').addEventListener('click', toggleHud);
$('hudRail').addEventListener('click', toggleHud);
function toggleBare() {
  closeHelp();
  settings.ui.bare = !settings.ui.bare;
  syncLayout();
  saveSettings(settings);
  $(settings.ui.bare ? 'restoreUi' : 'panelToggle').focus({ preventScroll: true });
}
$('btnFreeze').addEventListener('click', () => {
  state.frozen = !state.frozen;
  $('btnFreeze').classList.toggle('on', state.frozen);
});
$('btnHide').addEventListener('click', toggleBare);
$('restoreUi').addEventListener('click', toggleBare);

const srcButtons = { synth: $('srcSynth'), input: $('srcInput'), file: $('srcFile') };
let startSeq = 0;  // a start that finishes after a newer one has begun is dropped
async function start(kind, arg) {
  const seq = ++startSeq;
  input.waiting = false;
  try {
    let want = null;
    if (kind === 'synth') {
      if ((await audio.startSynth(arg)) === null) return;
    } else if (kind === 'input') {
      want = await audio.startInput(arg, inputLost);
      if (want === null) return;
    } else if (kind === 'file') {
      if ((await audio.startFile(arg)) === null) return;
    }
    if (seq !== startSeq) return;
    input.want = kind === 'input' ? want : null;
    if (kind === 'input') fillDevices(arg);
    showNotice(null);
    state.running = true;
    clearMaxHold();
    peaks.clear();
    for (const [k, btn] of Object.entries(srcButtons)) btn.classList.toggle('on', k === kind);
    state.test = kind === 'synth' ? arg : 0;
    state.status = kind === 'file' && arg?.name ? { text: arg.name } : kind === 'synth' ? { key: 'test', vars: { n: arg } } : { key: kind };
    showStatus();
    showTestButton();
  } catch (err) {
    if (seq !== startSeq) return;
    state.status = { key: 'cannotOpen', vars: { msg: err.message } };
    showStatus();
  }
}
// The test sound button cycles: off, test audio 1, test audio 2, off.
$('srcSynth').addEventListener('click', () => {
  const next = (state.test + 1) % 3;
  if (next) { start('synth', next); return; }
  startSeq++;
  audio.stop();
  input.want = null;
  input.waiting = false;
  showNotice(null);
  state.test = 0;
  state.status = { key: 'stopped' };
  for (const btn of Object.values(srcButtons)) btn.classList.remove('on');
  showStatus();
  showTestButton();
});
function showTestButton() {
  $('srcSynth').textContent = state.test ? t('test', { n: state.test }) : t('synth');
}
$('srcInput').addEventListener('click', () => start('input'));
$('srcFile').addEventListener('click', () => $('file').click());
$('file').addEventListener('change', (e) => e.target.files[0] && start('file', e.target.files[0]));
bindSelect('device', (id) => start('input', id));

// ---- Input device unplugged and plugged back in ----
// Same behaviour and wording as the desktop app: the source stays on the input, the status
// says it is waiting, a notice on the picture says so too (seen with the panel collapsed), and
// once the same device is back it reopens by itself. Looked for on every devicechange and,
// in case a browser misses one, once a second while waiting.
const input = { want: null, waiting: false, trying: false };
const RECONNECT_POLL_MS = 1000;
const RESTORED_NOTICE_MS = 3000;
let noticeTimer = 0;
let glLost = false;  // once the GPU has dropped the context, only a reload brings the picture back
function showNotice(key, ms = 0) {
  if (glLost && key !== 'glLost') return;
  clearTimeout(noticeTimer);
  const el = $('notice');
  if (key) el.dataset.i18n = key;
  else delete el.dataset.i18n;
  el.textContent = key ? t(key) : '';
  el.hidden = !key;
  if (key && ms) noticeTimer = setTimeout(() => showNotice(null), ms);
}
// The device list comes later, or never (listInputs): until it does the picker stays hidden.
async function fillDevices(chosen) {
  const devices = await listInputs();
  const sel = $('device');
  if (!devices?.length || !input.want) return;
  sel.replaceChildren(...devices.map((d) => new Option(d.label || d.deviceId.slice(0, 8), d.deviceId)));
  const current = chosen || findInput(devices, input.want)?.deviceId;
  if (current && devices.some((d) => d.deviceId === current)) sel.value = current;
  sel.hidden = false;
}
function inputLost() {
  if (!input.want) return;
  input.waiting = true;
  state.status = { key: 'waitingDevice' };
  showStatus();
  showNotice('deviceLost');
  reconnect();
}
async function reconnect() {
  if (!input.waiting || input.trying) return;
  input.trying = true;
  try {
    const devices = await listInputs();
    if (!input.waiting) return;
    // Without a device list (that phone), try the old id directly; a missing device just fails.
    const found = devices ? findInput(devices, input.want)?.deviceId : input.want.id;
    if (!found) return;
    const seq = ++startSeq;
    const want = await audio.startInput(found, inputLost).catch(() => null);
    if (!want || seq !== startSeq || !input.waiting) return;
    input.want = want;
    input.waiting = false;
    state.status = { key: 'input' };
    showStatus();
    showNotice('deviceBack', RESTORED_NOTICE_MS);
    fillDevices(found);
  } finally {
    input.trying = false;
  }
}
// A lost context is not restored (that would mean rebuilding every buffer and texture); say
// so instead of leaving a blank picture.
canvas.addEventListener('webglcontextlost', () => {
  showNotice('glLost');
  glLost = true;
});
navigator.mediaDevices?.addEventListener?.('devicechange', () => {
  if (input.waiting) reconnect();
  else if (input.want) fillDevices($('device').value);
});
setInterval(() => { if (input.waiting) reconnect(); }, RECONNECT_POLL_MS);

const viewButtons = { front: $('vFront'), top: $('vTop'), perspective: $('vPersp') };
function setView(name) {
  if (state.flat && name !== 'front') return;
  camera.setView(name);
}
for (const [name, el] of Object.entries(viewButtons)) el.addEventListener('click', () => setView(name));

window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { closeHelp(); if (document.body.classList.contains('panel-overlay')) setPanelOpen(false); }
  if (e.target.tagName === 'INPUT' || e.target.tagName === 'SELECT') return;
  if (e.metaKey || e.ctrlKey || e.altKey) return;
  const k = e.key.toLowerCase();
  if (e.key === '1') setView('front');
  if (e.key === '2') setView('top');
  if (e.key === '3') setView('perspective');
  if (e.key === ' ') { state.frozen = !state.frozen; $('btnFreeze').classList.toggle('on', state.frozen); e.preventDefault(); }
  if (k === 'h') toggleBare();
  if (k === 'm') setMonitorVisible(!settings.monitor.visible);
  if (k === 'c') clearMaxHold();
  if (MOVE_KEYS[e.code]) { held.add(e.code); e.preventDefault(); }
});
window.addEventListener('keyup', (e) => held.delete(e.code));
window.addEventListener('blur', () => { held.clear(); closeHelp(); });

// Held keys move the camera every frame: arrows or WASD pan, with Shift they orbit;
// + and - zoom. Same speeds as the gamepad sticks.
const MOVE_KEYS = {
  ArrowLeft: [-1, 0], KeyA: [-1, 0], ArrowRight: [1, 0], KeyD: [1, 0],
  ArrowUp: [0, 1], KeyW: [0, 1], ArrowDown: [0, -1], KeyS: [0, -1],
  Equal: [0, 0, -1], NumpadAdd: [0, 0, -1], Minus: [0, 0, 1], NumpadSubtract: [0, 0, 1],
};
const held = new Set();
let shiftHeld = false;
window.addEventListener('keydown', (e) => { shiftHeld = e.shiftKey; });
window.addEventListener('keyup', (e) => { shiftHeld = e.shiftKey; });
function keyMotion(dt) {
  let x = 0, y = 0, z = 0;
  for (const code of held) {
    const [dx, dy, dz = 0] = MOVE_KEYS[code];
    x += dx; y += dy; z += dz;
  }
  if (!x && !y && !z) return;
  if (shiftHeld && !state.flat) camera.orbit(-x * 420 * dt, -y * 420 * dt);
  else if (x || y) camera.panBy(-x * 520 * dt, y * 520 * dt, cssH);
  if (z) camera.zoomBy(Math.exp(z * 1.6 * dt));
}

// ---- Gestures: one finger orbits, two fingers pinch and pan, double tap resets ----
const pointers = new Map();
let pinch = null;
// Middle-button drag orbits like left; stop the browser's middle-click autoscroll.
canvas.addEventListener('mousedown', (e) => { if (e.button === 1) e.preventDefault(); });
canvas.addEventListener('auxclick', (e) => e.preventDefault());
canvas.addEventListener('pointerdown', (e) => {
  canvas.setPointerCapture(e.pointerId);
  pointers.set(e.pointerId, { x: e.clientX, y: e.clientY, button: e.button, shift: e.shiftKey });
  if (pointers.size === 2) pinch = pinchState();
});
canvas.addEventListener('pointermove', (e) => {
  const p = pointers.get(e.pointerId);
  if (!p) { readout(e); return; }
  const dx = e.clientX - p.x, dy = e.clientY - p.y;
  p.x = e.clientX; p.y = e.clientY;
  if (pointers.size === 1) {
    if (p.button === 2 || p.shift) camera.panBy(dx, dy, cssH);
    else if (!state.flat) camera.orbit(dx, dy);
  } else if (pointers.size === 2 && pinch) {
    const now = pinchState();
    camera.zoomBy(pinch.dist / now.dist);
    camera.panBy(now.cx - pinch.cx, now.cy - pinch.cy, cssH);
    pinch = now;
  }
  $('readout').hidden = true;
});
const release = (e) => { pointers.delete(e.pointerId); pinch = pointers.size === 2 ? pinchState() : null; };
canvas.addEventListener('pointerup', release);
canvas.addEventListener('pointercancel', release);
canvas.addEventListener('pointerleave', () => { $('readout').hidden = true; state.hoverBand = null; });
canvas.addEventListener('contextmenu', (e) => e.preventDefault());
canvas.addEventListener('dblclick', () => setView(state.flat ? 'front' : 'perspective'));
canvas.addEventListener('wheel', (e) => {
  e.preventDefault();
  camera.zoomBy(Math.exp(e.deltaY * (e.ctrlKey ? 0.01 : 0.0015)));
}, { passive: false });
function pinchState() {
  const [a, b] = [...pointers.values()];
  return { dist: Math.max(10, Math.hypot(a.x - b.x, a.y - b.y)), cx: (a.x + b.x) / 2, cy: (a.y + b.y) / 2 };
}

// ---- Readout under the cursor: frequency, note, level, time ----
let lastInverse = null;
const readoutEl = $('readout');
const readoutMain = document.createElement('div');
readoutMain.className = 'readout-main';
const readoutSub = document.createElement('div');
readoutSub.className = 'readout-sub';
readoutEl.append(readoutMain, readoutSub);

const fmtSignedDb = (v) => `${v < -0.05 ? `−${Math.abs(v).toFixed(1)}` : Math.abs(v).toFixed(1)}\u202fdB`;
const fmtHz = (hz) => (hz >= 1000 ? `${(hz / 1000).toFixed(hz >= 10000 ? 1 : 2)}\u202fkHz` : `${hz.toFixed(hz < 100 ? 1 : 0)}\u202fHz`);
const fmtCents = (c) => (c > 0 ? `+${c}¢` : c < 0 ? `−${-c}¢` : '±0¢');

function readout(e) {
  if (!lastInverse) return;
  const r = canvas.getBoundingClientRect();
  const nx = ((e.clientX - r.left) / r.width) * 2 - 1;
  const ny = 1 - ((e.clientY - r.top) / r.height) * 2;
  const near = transform(lastInverse, nx, ny, -1), far = transform(lastInverse, nx, ny, 1);
  const a = near.slice(0, 3).map((v) => v / near[3]), b = far.slice(0, 3).map((v) => v / far[3]);
  const dir = b.map((v, i) => v - a[i]);
  const { width: w, height: h, depth: d, tail } = LOOK.size;
  let line1 = null, line2 = null;
  if (camera.current.pitch > 12 && Math.abs(dir[1]) > 1e-6) {
    const s = -a[1] / dir[1];
    const x = a[0] + dir[0] * s, z = a[2] + dir[2] * s;
    const u = x / w + 0.5, t = (0.5 - z / d) / tail;
    if (u >= 0 && u <= 1 && t >= 0 && t <= 1) {
      const band = uToBand(u, state.mel), hz = bandHz(band), age = t * spanRows();
      const db = renderer.sample(band, age);
      const n = noteName(hz);
      state.hoverBand = Math.round(band);
      line1 = `${n.name} ${fmtCents(n.cents)}  ${fmtHz(hz)}`;
      line2 = `${fmtSignedDb(db)}  −${(age / state.rowRate).toFixed(2)}\u202fs`;
    }
  } else if (Math.abs(dir[2]) > 1e-6) {
    const s = (d / 2 - a[2]) / dir[2];
    const x = a[0] + dir[0] * s, y = a[1] + dir[1] * s;
    const u = x / w + 0.5, level = y / h;
    if (u >= 0 && u <= 1 && level >= 0 && level <= 1) {
      const band = uToBand(u, state.mel), hz = bandHz(band);
      const db = LOOK.db.floor + level * (LOOK.db.ceiling - LOOK.db.floor);
      const n = noteName(hz);
      state.hoverBand = Math.round(band);
      line1 = `${n.name} ${fmtCents(n.cents)}  ${fmtHz(hz)}`;
      line2 = fmtSignedDb(db);
    }
  }
  readoutEl.hidden = !line1;
  if (!line1) state.hoverBand = null;
  if (line1) {
    readoutMain.textContent = line1;
    readoutSub.textContent = line2;
    const boxW = readoutEl.offsetWidth || 156;
    const boxH = readoutEl.offsetHeight || 48;
    const rx = Math.min(r.width - boxW - 8, Math.max(8, e.clientX - r.left + 14));
    const ry = Math.min(r.height - boxH - 8, Math.max(8, e.clientY - r.top + 14));
    readoutEl.style.transform = `translate(${rx.toFixed(1)}px, ${ry.toFixed(1)}px)`;
  }
}

// ---- Frame loop ----
let last = performance.now();

// The live spectrum follows the envelope of the analysis: a running maximum over half a
// semitone each side, then a triangular average of the same width. A peak keeps its exact
// level (the maximum is flat across the whole average there), while the narrow dips between
// neighbouring partials and the bin-to-bin flicker of noise fill in, which up high would
// otherwise hang below the curve as a row of needles. The waterfall keeps the raw rows.
function envelope(src, dst, scratch, r) {
  const n = src.length;
  if (r < 1) { dst.set(src); return; }
  for (let k = 0; k < n; k++) {
    let m = src[k];
    for (let i = Math.max(0, k - r), end = Math.min(n - 1, k + r); i <= end; i++) if (src[i] > m) m = src[i];
    scratch[k] = m;
  }
  for (let k = 0; k < n; k++) {
    let sum = 0, weights = 0;
    for (let i = -r; i <= r; i++) {
      const w = r + 1 - Math.abs(i);
      sum += w * scratch[Math.min(n - 1, Math.max(0, k + i))];
      weights += w;
    }
    dst[k] = sum / weights;
  }
}

// Half a semitone each side, in data bands: none at one band per semitone (the phone's
// format, nothing between two semitones to fill), then 1, 2 and 4. Same as Rust's
// DisplaySpectrum::smooth; web_parity.rs checks it.
function envelopeRadius(sub) {
  return Math.floor(sub / 2);
}

function updateLive(dt) {
  envelope(state.raw, state.live, state.scratch, envelopeRadius(state.sub));
  for (let k = 0; k < state.live.length; k++) {
    const v = state.live[k];
    state.shown[k] = v >= state.shown[k] ? v : Math.max(v, state.shown[k] - RELEASE_DB_PER_S * dt);
    if (v >= state.peak[k]) { state.peak[k] = v; state.peakAge[k] = 0; }
    else if ((state.peakAge[k] += dt) > PEAK_HOLD_S) state.peak[k] = Math.max(state.shown[k], state.peak[k] - PEAK_FALL_DB_PER_S * dt);
    if (state.shown[k] > state.maxHold[k]) state.maxHold[k] = state.shown[k];
  }
}

// Piano strip size for this frame. The Hz labels drop below it and the octave names sit in it.
let keyDrop = null;
function keyStrip() {
  const K = LOOK.keys;
  const strip = { gapPx: K.gapPx, heightPx: cssH < 300 ? K.heightPxSmall : K.heightPx, hoverBand: state.hoverBand };
  const on = settings.spectrum.piano;
  const drop = on ? `${strip.gapPx + strip.heightPx + 4}px ${strip.gapPx + strip.heightPx / 2}px` : '5px 0px';
  if (drop !== keyDrop) {
    keyDrop = drop;
    const [hz, key] = drop.split(' ');
    $('labels').style.setProperty('--hz-drop', hz);
    $('labels').style.setProperty('--key-drop', key);
  }
  return on ? strip : null;
}

function frame(now) {
  const elapsed = (now - last) / 1000;
  last = now;
  monitor.frame(now, elapsed);
  if (renderScale.update(elapsed, monitor.gpuMs, state.running && !state.paused && !state.frozen && !document.hidden)) resizeCanvas();
  if (!state.paused) tick(Math.min(0.1, elapsed));
  requestAnimationFrame(frame);
}

// One simulation and render step. Exposed so captures and tests can step at a fixed rate.
function tick(dt) {
  const t0 = performance.now();

  // Data rows at a fixed audio rate, as many as came due this frame (several at high rates).
  // Each goes into the history; the live spectrum shows the loudest of them per band, so a
  // short hit between two display frames still reaches it. The phone's data has 0.5 dB steps.
  let rows = 0, rowsMs = 0;
  const ta = performance.now();
  state.phase = !state.running ? 0 : audio.pull(dt, (bands) => {
    if (state.phoneMode) for (let k = 0; k < bands.length; k++) bands[k] = Math.round(bands[k] * 2) / 2;
    if (rows++) for (let k = 0; k < bands.length; k++) state.raw[k] = Math.max(state.raw[k], bands[k]);
    else state.raw.set(bands);
    const tp = performance.now();
    for (let k = 0; k < bands.length; k++) state.bytes[k] = dbToByte(bands[k]);
    renderer.pushRow(state.bytes);
    state.rowsPushed++;
    rowsMs += performance.now() - tp;
  }, state.frozen);
  monitor.add('analysis', performance.now() - ta - rowsMs);
  const sp = settings.spectrum;
  const spectrumOn = settings.layers.spectrum;
  if (!state.frozen) {
    updateLive(dt);
    if (spectrumOn && sp.peaks) peaks.update(state.shown, state.live, dt, state.phoneMode ? null : (b) => audio.peakHz(b), state.sub);
  }

  // Spectrum alone fills the frame like a 2D analyser: the level axis grows toward the
  // frame's shape, and shrinks back when the waterfall returns.
  const targetHeight = state.flat
    ? Math.min(1.5, Math.max(BASE_HEIGHT, LOOK.size.width * (cssH / cssW) * 0.84))
    : BASE_HEIGHT;
  // Low levels: the floor glides down to -120 dB, the terrain grows out of it.
  const targetFloor = settings.analysis.lowLevel ? LOW_FLOOR : BASE_FLOOR;
  // The frequency axis glides between semitone and mel; guides and labels follow it.
  const targetMel = settings.axis.scale === 'mel' ? 1 : 0;
  if (LOOK.size.height !== targetHeight || LOOK.db.floor !== targetFloor || state.mel !== targetMel) {
    const ease = 1 - Math.exp(-dt / 0.08);
    const m = state.mel + (targetMel - state.mel) * ease;
    state.mel = Math.abs(targetMel - m) < 1e-3 ? targetMel : m;
    renderer.melAxis = state.mel;
    const h = LOOK.size.height + (targetHeight - LOOK.size.height) * ease;
    LOOK.size.height = Math.abs(targetHeight - h) < 1e-3 ? targetHeight : h;
    const f = LOOK.db.floor + (targetFloor - LOOK.db.floor) * ease;
    LOOK.db.floor = Math.abs(targetFloor - f) < 0.05 ? targetFloor : f;
    renderer.buildGuides(state.seconds);
    labels.build(state.seconds, state.mel);
  }

  // Game controller: sticks orbit and pan, triggers zoom, A B Y pick views, X freezes.
  const pad = gamepad.poll(dt);
  if (pad) {
    if (!state.flat && (pad.orbit[0] || pad.orbit[1])) camera.orbit(pad.orbit[0], pad.orbit[1]);
    if (pad.pan[0] || pad.pan[1]) camera.panBy(pad.pan[0], pad.pan[1], cssH);
    if (pad.zoom !== 1) camera.zoomBy(pad.zoom);
    if (pad.down.has('a')) setView('front');
    if (pad.down.has('b')) setView('top');
    if (pad.down.has('y')) setView('perspective');
    if (pad.down.has('x')) state.frozen = !state.frozen;
  }
  keyMotion(dt);
  if (gamepad.name !== shownPad) {
    shownPad = gamepad.name;
    $('gamepad').hidden = !shownPad;
    $('gamepad').textContent = shownPad ? t('gamepad', { name: shownPad }) : '';
  }

  camera.update(dt);
  const aspect = cssW / cssH;
  const { viewProj, inverse, back } = camera.matrices(aspect);
  lastInverse = inverse;
  // The tilt belongs to the flat front view and eases in and out with it.
  const tilt = sp.tilt ? TILT_DB_PER_OCTAVE * viewMetrics(inverse).frontness : 0;
  const tu = performance.now();
  if (spectrumOn) {
    renderer.updateFront(renderer.frontLine, state.shown);
    renderer.updateFront(renderer.peakLine, state.peak);
    renderer.updateFront(renderer.curtain, state.shown);
    if (sp.maxHold) renderer.updateFront(renderer.maxLine, state.maxHold);
  }
  const keys = keyStrip();
  if (keys) {
    state.keyLit.fill(0);
    for (const p of state.labelled) {
      const k = Math.round(p.band);
      state.keyLit[k] = Math.max(state.keyLit[k], p.alpha * (p.main ? 1 : 0.6));
    }
    renderer.updateKeys(state.keyLit);
  }
  renderer.flushHistory();
  const td = performance.now();

  gl.viewport(0, 0, canvas.width, canvas.height);
  monitor.gpuBegin();
  renderer.draw({
    viewProj,
    phase: state.phase,
    rowRate: state.rowRate,
    spanRows: spanRows(),
    layers: settings.layers,
    maxHold: sp.maxHold,
    keys,
    tilt,
  });
  monitor.gpuEnd();
  labels.update(viewProj, back, camera.current.pitch, cssW, cssH, { time: settings.layers.waterfall, keys: !!keys });
  // Peak labels fade out toward the top view, like the peak-hold line, and behind the terrain.
  const topness = Math.min(1, Math.max(0, (camera.current.pitch - 62) / 24));
  state.labelled = peakLabels.update(peaks.tracks, viewProj, LOOK, cssW, cssH,
    spectrumOn && sp.peaks ? (1 - topness) * frontFacing(back) : 0, tilt, state.mel);

  for (const [name, el] of Object.entries(viewButtons)) el.classList.toggle('on', camera.view === name);
  $('btnFreeze').classList.toggle('on', state.frozen);

  const te = performance.now();
  monitor.add('upload', rowsMs + (td - tu));
  monitor.add('draw', te - td);
  monitor.add('total', te - t0);
  if (settings.monitor.visible) {
    monitor.render(settings.monitor, { dt, canvas, renderer, audio, rowsPushed: state.rowsPushed, frozen: state.frozen, renderScale });
  }
}

rebuild();
setFrame('full');
applyLanguage(settings.ui.lang);
applyLayers();
applyAnalysis();
requestAnimationFrame(frame);
window.spectrumLab = { state, settings, camera, renderer, audio, monitor, peaks, renderScale, LOOK, setView, tick, rebuild, switchStyle, styles: Object.keys(STYLES), get currentStyle() { return currentStyle; } };
const requestedStyle = new URLSearchParams(location.search).get('visualStyle');
const startupStyle = Object.hasOwn(STYLES, requestedStyle) ? requestedStyle : settings.visual.name;
if (startupStyle !== currentStyle) await switchStyle(startupStyle);

function rememberStyle() {
  settings.styleOptions[currentStyle] = { style: LOOK.style };
  settings.visual.name = currentStyle; saveSettings(settings);
}
async function switchStyle(name) {
  if (!Object.hasOwn(STYLES, name) || name === currentStyle || switchingStyle) return;
  switchingStyle = true; $('visualStyle').disabled = true;
  let next;
  try {
    const spec = await loadStyle(name);
    const options = settings.styleOptions[name];
    if (options && Object.hasOwn(spec.look.styles, options.style)) spec.look.style = options.style;
    next = await spec.Renderer.create(gl, spec.look);
    if (next.sub !== state.sub) next.setBands(state.sub);
    // Adopt the live CPU history, including rows received while shader loading was in flight.
    next.historyStore = renderer.historyStore; next.history = renderer.history;
    next.rowsPushed = renderer.rowsPushed; next.historyStore.dirty.fill(HISTORY_ROWS);
    const old = renderer;
    rememberStyle();
    for (const key of Object.keys(LOOK)) delete LOOK[key];
    Object.assign(LOOK, spec.look); next.look = LOOK; renderer = next;
    BASE_HEIGHT = LOOK.size.height; BASE_FLOOR = LOOK.db.floor;
    currentStyle = name; $('visualStyle').value = name;
    $('style').replaceChildren(...Object.entries(LOOK.styles).map(([key, item]) => new Option(item.label || key, key)));
    $('style').value = LOOK.style;
    let sheet = $('styleSheet');
    if (!sheet) { sheet = document.createElement('link'); sheet.id = 'styleSheet'; sheet.rel = 'stylesheet'; document.head.append(sheet); }
    sheet.disabled = name !== 'dadaspec';
    if (!sheet.disabled) sheet.href = `styles/${name}/stage.css`;
    rebuild();
    if (camera.view !== 'free') camera.setView(state.flat ? 'front' : camera.view);
    window.spectrumLab.renderer = renderer;
    old.dispose(); rememberStyle();
  } catch (error) { if (next && next !== renderer) next.dispose(); throw error; }
  finally { switchingStyle = false; $('visualStyle').disabled = false; }
}
