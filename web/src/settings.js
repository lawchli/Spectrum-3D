// Display and monitor switches, remembered in this browser. Kept apart from the panel
// markup so a redesigned panel only has to rebind them.

import { defaultLang } from './i18n.js';
import { STYLE_NAMES } from './style-names.js';

const KEY = 'spectrum-visualizer.settings.v1';

export const DEFAULTS = {
  // 'auto' follows the viewport until the user explicitly opens/collapses the panel or the
  // monitor: collapsed on a phone-sized viewport, open elsewhere. On a phone-sized viewport an
  // opened panel covers the picture and is never remembered (main.js syncLayout).
  ui: { lang: '', panel: 'auto', hud: 'auto', bare: false },
  visual: { name: 'tungsten' },
  styleOptions: {},
  render: { scale: '1' },
  layers: { spectrum: true, waterfall: true },  // at least one stays on
  axis: { scale: 'log' },  // frequency axis: 'log' (a semitone everywhere the same width) or 'mel'
  spectrum: { peaks: false, maxHold: false, piano: false, tilt: false },  // off until turned on
  // Rows a second, bands per semitone (0: not chosen yet, see DEFAULT_SUB), floor at -120 dB.
  analysis: { rowRate: 60, sub: 0, lowLevel: false },
  monitor: { visible: true, frame: true, gpu: true, cpu: true, memory: true, audio: true },
};

// Bands per semitone until the user picks: four on a desktop-sized window, so the top view
// and the front curve are smooth; one (the phone's own format, a quarter of the vertices) on
// a small screen.
export const DEFAULT_SUB = globalThis.matchMedia?.('(max-width: 900px)').matches ? 1 : 4;

export function loadSettings() {
  const s = structuredClone(DEFAULTS);
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) || '{}');
    for (const group of Object.keys(s)) {
      for (const k of Object.keys(s[group])) {
        if (typeof saved[group]?.[k] === typeof s[group][k]) s[group][k] = saved[group][k];
      }
    }
  } catch { /* private window or blocked storage: defaults */ }
  try {
    const stored = JSON.parse(localStorage.getItem(KEY) || '{}').styleOptions;
    for (const name of STYLE_NAMES) {
      const item = stored?.[name];
      if (item && typeof item.style === 'string') s.styleOptions[name] = { style: item.style };
    }
  } catch {}
  if (!s.analysis.sub) s.analysis.sub = DEFAULT_SUB;
  if (!['log', 'mel'].includes(s.axis.scale)) s.axis.scale = 'log';
  if (!s.layers.spectrum && !s.layers.waterfall) s.layers.waterfall = true;
  if (!STYLE_NAMES.includes(s.visual.name)) s.visual.name = 'tungsten';
  if (!s.ui.lang) s.ui.lang = defaultLang();
  if (!['auto', 'open', 'collapsed'].includes(s.ui.panel)) s.ui.panel = 'auto';
  if (!['auto', 'open', 'collapsed'].includes(s.ui.hud)) s.ui.hud = 'auto';
  try {
    // Before 2026-11-14 the monitor had only hudCollapsed (true or false, false by default).
    const ui = JSON.parse(localStorage.getItem(KEY) || '{}').ui;
    if (ui && !('hud' in ui) && ui.hudCollapsed === true) s.ui.hud = 'collapsed';
  } catch {}
  return s;
}

export function saveSettings(s) {
  try { localStorage.setItem(KEY, JSON.stringify(s)); } catch { /* not remembered, still works */ }
}
