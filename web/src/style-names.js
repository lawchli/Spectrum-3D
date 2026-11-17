// The one list of visual styles, [key, label]. Tungsten, Mel and Green are the shared renderer in
// three colourways (look.js variants); Dadaspec has its own web/styles/dadaspec/. Each style has
// one fixed palette. Settings, the style switcher and the check scripts all read the list here.
export const STYLE_LIST = [
  ['tungsten', 'Tungsten'], ['mel', 'Mel'], ['green', 'Green'], ['dadaspec', 'Dadaspec'],
];
export const STYLE_NAMES = STYLE_LIST.map(([key]) => key);
