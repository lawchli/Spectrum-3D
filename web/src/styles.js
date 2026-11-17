import { Renderer } from './renderer.js';
import { LOOK } from './look.js';
import { STYLE_LIST } from './style-names.js';

export const STYLES = Object.fromEntries(STYLE_LIST.map(([key, label]) => [key, { label }]));
const base = structuredClone(LOOK);

// The shared look in one of its colourways: the palette of that name, and the variant's
// own values laid over the matching groups.
function variantLook(name) {
  const look = structuredClone(base);
  look.palette = name;
  for (const [key, value] of Object.entries(look.variants[name])) {
    look[key] = typeof value === 'object' ? { ...look[key], ...value } : value;
  }
  return look;
}

export async function loadStyle(name) {
  if (!Object.hasOwn(STYLES, name)) name = 'tungsten';
  if (Object.hasOwn(base.variants, name)) return { Renderer, look: variantLook(name) };
  const [renderer, look] = await Promise.all([
    import(`../styles/${name}/renderer.js`), import(`../styles/${name}/look.js`),
  ]);
  return { Renderer: renderer.Renderer, look: structuredClone(look.LOOK) };
}
