import { createTheme } from '@mantine/core';

export const theme = createTheme({
  primaryColor: 'indigo',
  defaultRadius: 'md',
  defaultGradient: { from: 'indigo', to: 'cyan', deg: 135 },
  fontFamily:
    '"Zen Kaku Gothic New", "Hiragino Kaku Gothic ProN", "Noto Sans JP", system-ui, sans-serif',
  headings: {
    fontFamily: '"Zen Kaku Gothic New", "Hiragino Kaku Gothic ProN", system-ui, sans-serif',
    fontWeight: '800',
  },
});
