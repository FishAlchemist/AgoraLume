import { Button, Group, Modal, Stack, Text } from '@mantine/core';
import { useTranslation } from 'react-i18next';
import { useUi } from '../store/ui';

/** Global confirmation dialog, driven by useUi().askConfirm — replaces window.confirm. */
export function ConfirmDialog() {
  const { t } = useTranslation();
  const confirm = useUi((s) => s.confirm);
  const closeConfirm = useUi((s) => s.closeConfirm);

  const handleConfirm = () => {
    confirm?.onConfirm();
    closeConfirm();
  };

  return (
    <Modal
      opened={Boolean(confirm)}
      onClose={closeConfirm}
      size="sm"
      centered
      title={confirm?.title ?? t('common.confirmTitle')}
    >
      <Stack gap="lg">
        <Text size="sm">{confirm?.message}</Text>
        <Group justify="flex-end">
          <Button variant="default" onClick={closeConfirm}>
            {t('common.cancel')}
          </Button>
          <Button color={confirm?.danger ? 'red' : undefined} onClick={handleConfirm}>
            {confirm?.confirmLabel ?? t('common.confirm')}
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}
