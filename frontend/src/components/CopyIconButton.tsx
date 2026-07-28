import { ActionIcon, CopyButton, Tooltip } from '@mantine/core';
import { IconCheck, IconCopy } from '@tabler/icons-react';
import { useTranslation } from 'react-i18next';

interface Props {
  /** The text placed on the clipboard when clicked. */
  value: string;
  /** Icon size in px (default 16). */
  size?: number;
}

/**
 * A small icon button that copies `value` to the clipboard, flipping to a check
 * for a moment after a successful copy. Shared by the debug panel and the
 * persona prompt preview so the "copy this block" affordance stays identical.
 */
export function CopyIconButton({ value, size = 16 }: Props) {
  const { t } = useTranslation();
  return (
    <CopyButton value={value}>
      {({ copied, copy }) => (
        <Tooltip label={copied ? t('common.copied') : t('common.copy')} withArrow>
          <ActionIcon
            variant="subtle"
            color={copied ? 'teal' : 'gray'}
            onClick={copy}
            aria-label={t('common.copy')}
          >
            {copied ? <IconCheck size={size} /> : <IconCopy size={size} />}
          </ActionIcon>
        </Tooltip>
      )}
    </CopyButton>
  );
}
