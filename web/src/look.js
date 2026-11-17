// Visual specification for the 3D acoustic spectrum analyzer.
// Designed for a 4.6" 1280x720 landscape monolith on a dark synth desk: pure #000000 floor,
// restrained precision graticules, and OKLab energy ramps calibrated to real synth headroom.

export const LOOK = {
  background: '#000000',

  // World dimensions of the framed window (x frequency, y level, z time).
  // Proportioned so Front (2.82:1), Top (1.67:1), and 3D (+34° yaw) all fill a 16:9 screen
  // cleanly without clipping axis labels in 16:9, 9:16, or 360x200 frames.
  size: { width: 2.20, height: 0.78, depth: 1.32, tail: 4.5 },

  // The waterfall keeps its full brightness across the framed part (the first 1/tail of its
  // length) and then fades smoothly to exact #000000 at v_t = 0.96, without a visible edge.
  fog: { start: 0.21, power: 1.45 },

  db: { floor: -96, ceiling: 0 },

  // 8-stop OKLab ramps from -96 dBFS to 0 dBFS, one per colourway of this renderer
  // (see variants below). Stops 0..1 (-96..-82 dB) remain near-zero black so ADC/room noise
  // never tints the stage; stop 6 (-14 dB) is the nominal peak range of a synth at full output.
  palettes: {
    tungsten: ['#000000', '#020305', '#0b1219', '#1d303f', '#476d82', '#9fc1cd', '#f0bb68', '#fff3da'],
    // The spectrogram of audio repair editors (iZotope RX and the like): black through deep
    // blue for quiet detail, then orange to pale yellow for what is loud.
    mel:      ['#000000', '#030712', '#0a1f4d', '#16479c', '#6a3f93', '#d1532e', '#f8a024', '#fff4bf'],
    // One hue, the green of a classic analyser screen.
    green:    ['#000000', '#010601', '#04200a', '#0a4a17', '#14852a', '#2fc94a', '#8df59a', '#eaffea'],
  },
  // How much the bass is warmed and the highs cooled on top of the ramp. Only Tungsten does.
  paletteRegister: { tungsten: 0.85, mel: 0, green: 0 },
  palette: 'tungsten',

  // The colourways the style switcher offers for this renderer (styles.js): the palette of
  // the same name, plus whatever else each one changes in this look.
  variants: {
    tungsten: {},
    mel: { front: { trace: '#fff7e6', peak: '#8fa3c8', headroom: '#ffb347', maxHold: '#c9d4ea' } },
    green: { style: 'contours', front: { trace: '#e6ffe9', peak: '#5fbf70', headroom: '#f2a649', maxHold: '#a8e6b0' } },
  },

  // Terrain rendering modes (all evaluated analytically in surface.frag in a single pass).
  // Labels use the usual names from 3D plotting (MATLAB surfl, mesh, surf) and data
  // visualisation (ridgeline plot), untranslated in both languages.
  styles: {
    sculpt: {
      label: 'Lit Surface',
      surfaceMix: 1.0,
      sliceStrength: 0.44,
      sliceEvery: 3.0,
      ribStrength: 0.0,
      ribEvery: 2.0,
      contourStrength: 0.24,
      octaveStrength: 0.34,
      lineFloor: 0.16,
    },
    contours: {
      label: 'Ridgeline',
      surfaceMix: 0.0,
      sliceStrength: 0.96,
      sliceEvery: 2.5,
      ribStrength: 0.0,
      ribEvery: 2.0,
      contourStrength: 0.0,
      octaveStrength: 0.24,
      lineFloor: 0.32,
    },
    lattice: {
      label: 'Mesh',
      surfaceMix: 0.60,
      sliceStrength: 0.64,
      sliceEvery: 2.0,   // a line every 2 data rows and every semitone
      ribStrength: 0.56,
      ribEvery: 1.0,
      contourStrength: 0.0,
      octaveStrength: 0.42,
      lineFloor: 0.22,
    },
    pure: {
      label: 'Solid',
      surfaceMix: 1.0,
      sliceStrength: 0.0,
      sliceEvery: 3.0,
      ribStrength: 0.0,
      ribEvery: 2.0,
      contourStrength: 0.20,
      octaveStrength: 0.28,
      lineFloor: 0.14,
    },
  },
  style: 'sculpt',

  surface: {
    exposure: 1.16,  // overall terrain and curtain brightness
    ambient: 0.38,
    keyLight: [-0.42, 0.84, 0.48],
    rimLight: [0.55, 0.44, -0.66],
  },

  // Kept for buildMesh index generation compatibility.
  grid: { everyFreq: 2, everyTime: 3, color: '#d8e0e8', palette: 0.85, alpha: 0.50 },

  front: {
    trace: '#eef1f5',
    peak: '#93a0ad',
    headroom: '#f2a649',
    maxHold: '#b9c6d3',
  },

  // Piano strip under the frequency axis (one key per semitone band).
  keys: {
    heightPx: 11,
    heightPxSmall: 7,  // frames under 300 px tall
    gapPx: 3,
    alpha: 0.9,
    white: '#262b32',
    black: '#0c0e11',
    lit: '#f2a649',
    hover: '#8d98a5',
    octave: '#5f6975',
  },

  guides: {
    railColor: '#b8c4d0',
    railAlpha: 0.30,
    floorColor: '#96a4b3',
    floorAlpha3d: 0.045,
    floorAlphaTop: 0.10,
    timeTickAlpha: 0.32,
    spineColor: '#b8c4d0',
    spineAlpha: 0.28,
    frontGridColor: '#b8c4d0',
    frontGridAlpha: 0.095,
    accentColor: '#f2a649',
    accentAlpha: 0.38,
    headroomLineAlpha: 0.15,
  },

  // Camera presets: fov 1° produces a flat orthographic-like chart via dolly zoom.
  views: {
    front:       { yaw: 0,   pitch: 0,    fov: 1.0, zoom: 1.06, pan: [-0.03, -0.025, 0] },
    top:         { yaw: 0,   pitch: 89.9, fov: 1.0, zoom: 1.05, pan: [0.035, 0, 0.045] },
    // The classic waterfall angle (2026-11-13): from the left front and low, about level with
    // the terrain, so the live spectrum on the front edge still reads like a 2D analyser.
    perspective: { yaw: -22, pitch: 15,   fov: 30,  zoom: 0.95, pan: [0, -0.02, 0.04] },
  },
  freeFov: 30,
  motion: {
    presetSeconds: 0.36,
    dragSeconds: 0.045,
    pitchMin: -3,
    pitchMax: 89.9,
  },

  labels: {
    hz: [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000],
    hzCompact: [20, 100, 500, 2000, 10000, 20000],
    // On the mel axis the low decades sit within a few pixels of each other.
    hzMel: [100, 500, 1000, 2000, 3000, 5000, 7000, 10000, 15000, 20000],
    hzMelCompact: [100, 1000, 3000, 10000, 20000],
    db: [0, -12, -24, -48, -72, -96],
    dbCompact: [0, -24, -48, -72, -96],
    headroomDb: -12,
    seconds: 1,
  },
};
