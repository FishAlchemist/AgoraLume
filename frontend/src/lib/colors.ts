/** Mantine palette names offered as accent colors in the editors. */
export const ACCENT_COLORS = [
  'indigo',
  'violet',
  'grape',
  'blue',
  'cyan',
  'teal',
  'green',
  'lime',
  'yellow',
  'orange',
  'red',
  'pink',
  'gray',
] as const;

export type AccentColor = (typeof ACCENT_COLORS)[number];
