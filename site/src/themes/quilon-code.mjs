/**
 * The Quilon code themes: the claret ramp quilon.run renders samples in, as a
 * light and a dark Shiki theme. One hue family, not a rainbow — operators and
 * control symbols claret, types bordeaux, constants and numbers rose, strings
 * mauve, declared names plain foreground. Every colour clears 4.5:1 contrast
 * on its theme's code background (checked, not assumed).
 */

const light = {
  claret: '#a2194f',
  bordeaux: '#6e1230',
  rose: '#8a3d5c',
  mauve: '#7a4a63',
  comment: '#6e6a61',
  fg: '#1a1a1c',
  bg: '#f4f2ef',
};

const dark = {
  claret: '#e06d9c',
  bordeaux: '#e39db4',
  rose: '#d495ae',
  mauve: '#c9a3bd',
  comment: '#9b948d',
  fg: '#eee9e5',
  bg: '#241d22',
};

const tokenColors = (c) => [
  { scope: ['comment', 'punctuation.definition.comment'], settings: { foreground: c.comment } },
  { scope: ['keyword.operator', 'keyword.control', 'punctuation.definition.block'], settings: { foreground: c.claret } },
  { scope: ['entity.name.type', 'support.type', 'storage.type'], settings: { foreground: c.bordeaux } },
  { scope: ['support.type.builtin.unit'], settings: { foreground: c.rose } },
  { scope: ['constant', 'variable.other.constant'], settings: { foreground: c.rose } },
  { scope: ['string', 'punctuation.definition.string'], settings: { foreground: c.mauve } },
  { scope: ['constant.character.escape'], settings: { foreground: c.rose } },
  { scope: ['entity.name.function', 'entity.name.namespace', 'variable', 'meta.import'], settings: { foreground: c.fg } },
  { scope: ['punctuation.separator', 'punctuation.accessor'], settings: { foreground: c.fg } },
];

const theme = (name, type, c) => ({
  name,
  type,
  colors: {
    'editor.background': c.bg,
    'editor.foreground': c.fg,
  },
  settings: [{ settings: { foreground: c.fg } }],
  tokenColors: tokenColors(c),
});

export const quilonLight = theme('quilon-light', 'light', light);
export const quilonDark = theme('quilon-dark', 'dark', dark);
