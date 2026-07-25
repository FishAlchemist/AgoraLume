import { ColorSwatch, Group, Select, Text } from '@mantine/core';
import { ACCENT_COLORS } from '../lib/colors';

interface Props {
  label?: string;
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
}

/** Accent-color picker rendering a swatch beside each palette name. */
export function ColorSelect({ label, value, onChange, disabled }: Props) {
  return (
    <Select
      label={label}
      value={value}
      onChange={(v) => v && onChange(v)}
      disabled={disabled}
      data={ACCENT_COLORS.map((c) => ({ value: c, label: c }))}
      allowDeselect={false}
      leftSection={<ColorSwatch color={`var(--mantine-color-${value}-6)`} size={16} />}
      renderOption={({ option }) => (
        <Group gap="xs">
          <ColorSwatch color={`var(--mantine-color-${option.value}-6)`} size={16} />
          <Text size="sm">{option.label}</Text>
        </Group>
      )}
    />
  );
}
