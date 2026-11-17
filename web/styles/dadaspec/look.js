// Visual specification for the Dadaspec 3D acoustic spectrum style.
// Translates dadabots/dadaspec's perceptual RGB tri-band spectrogram grammar
// (Bass <200 Hz -> Red, Mid 200-1500 Hz -> Green, High >1500 Hz -> Blue, independently
// normalized per band so broadband energy saturates additively to chalk white while solo
// registers hold pure chroma even at full scale, paired with a perceptual gamma curve)
// into a real-time 3D spatial instrument over an endless #000000 obsidian stage.

export const LOOK = {
  background: '#000000',

  // Identical world proportions to the shared stage so view presets and style switches never jump.
  size: { width: 2.20, height: 0.78, depth: 1.32, tail: 4.5 },

  // The waterfall keeps its full brightness across the framed part (the first 1/tail of its
  // length) and then fades smoothly to exact #000000 at v_t = 0.96, without a visible edge.
  fog: { start: 0.21, power: 1.45 },

  db: { floor: -96, ceiling: 0 },

  // Dadaspec acoustic crossover boundaries in Hz (Low < 200 Hz, Mid 200..1500 Hz, High >= 1500 Hz).
  crossovers: [200, 1500],

  // 8-stop OKLab energy ramp, kept alongside the tri-band matrix for the base renderer.
  palettes: {
    dadaspec: ['#000000', '#020306', '#0c1622', '#1d3c4a', '#2fa86e', '#58d6c4', '#dcebf7', '#f6faff'],
  },
  palette: 'dadaspec',

  // Tri-band additive primaries, the canonical Dadaspec RGB: vermilion bass (<200 Hz), signal
  // emerald mid (200-1500 Hz), cobalt-azure high (>1500 Hz), adding up to cool chalk white.
  triad: { low: '#ff2b1c', mid: '#1ae668', high: '#3382ff', white: '#f4f8ff', chromaSat: 1.00, dispersion: 0.34 },

  // Four terrain modes reinterpreted around Dadaspec's discrete mel-channel ribbons & temporal seams.
  styles: {
    sculpt: {
      label: 'Lit Surface',
      surfaceMix: 1.0,
      sliceStrength: 0.44,
      sliceEvery: 2.0,
      ribStrength: 0.0,
      ribEvery: 1.0,
      channelEtch: 0.16,
      seamBoost: 0.58,
      contourStrength: 0.20,
      octaveStrength: 0.20,
      crossoverStrength: 0.46,
      lineFloor: 0.055,
    },
    contours: {
      label: 'Ridgeline',
      surfaceMix: 0.0,
      sliceStrength: 0.98,
      sliceEvery: 1.75,
      ribStrength: 0.0,
      ribEvery: 1.0,
      channelEtch: 0.10,
      seamBoost: 0.76,
      contourStrength: 0.0,
      octaveStrength: 0.16,
      crossoverStrength: 0.40,
      lineFloor: 0.075,
    },
    lattice: {
      label: 'Mesh',
      surfaceMix: 0.40,
      sliceStrength: 0.72,
      sliceEvery: 2.0,
      ribStrength: 0.64,
      ribEvery: 1.0,
      channelEtch: 0.08,
      seamBoost: 0.52,
      contourStrength: 0.0,
      octaveStrength: 0.32,
      crossoverStrength: 0.52,
      lineFloor: 0.065,
    },
    pure: {
      label: 'Solid',
      surfaceMix: 1.0,
      sliceStrength: 0.0,
      sliceEvery: 2.0,
      ribStrength: 0.0,
      ribEvery: 1.0,
      channelEtch: 0.12,
      seamBoost: 0.44,
      contourStrength: 0.14,
      octaveStrength: 0.15,
      crossoverStrength: 0.34,
      lineFloor: 0.045,
    },
  },
  style: 'sculpt',

  surface: {
    exposure: 1.10,
    ambient: 0.44,
    keyLight: [-0.38, 0.85, 0.46],
    rimLight: [0.54, 0.42, -0.68],
  },

  grid: { everyFreq: 2, everyTime: 2, color: '#dce6f2', palette: 0.85, alpha: 0.50 },

  front: {
    trace: '#f4f8ff',
    peak: '#9fb0c2',
    headroom: '#ffcf70',
    maxHold: '#c4d2e0',
  },

  keys: {
    heightPx: 11,
    heightPxSmall: 7,
    gapPx: 3,
    alpha: 0.92,
    white: '#22272f',
    black: '#0a0c10',
    lit: '#f4f8ff',
    hover: '#8e9cab',
    octave: '#596574',
  },

  guides: {
    railColor: '#c2cfde',
    railAlpha: 0.30,
    floorColor: '#90a0b2',
    floorAlpha3d: 0.042,
    floorAlphaTop: 0.095,
    timeTickAlpha: 0.34,
    spineColor: '#c2cfde',
    spineAlpha: 0.28,
    frontGridColor: '#b8c6d6',
    frontGridAlpha: 0.088,
    accentColor: '#ffcf70',
    accentAlpha: 0.40,
    headroomLineAlpha: 0.14,
    crossoverTickAlpha: 0.52,
    crossoverLineAlpha: 0.11,
  },

  // Preserve shared camera presets exactly as required.
  views: {
    front:       { yaw: 0,   pitch: 0,    fov: 1.0, zoom: 1.06, pan: [-0.03, -0.025, 0] },
    top:         { yaw: 0,   pitch: 89.9, fov: 1.0, zoom: 1.05, pan: [0.035, 0, 0.045] },
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
