import {
  ActionIcon,
  Alert,
  Button,
  Center,
  Group,
  Menu,
  Paper,
  PasswordInput,
  Stack,
  TextInput,
  Title,
  Tooltip,
} from '@mantine/core';
import { IconLanguage, IconX } from '@tabler/icons-react';
import { type FormEvent, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate } from 'react-router-dom';
import { UI_LANGUAGES } from '../i18n';
import { login } from '../lib/api/auth';
import { useAuth } from '../store/auth';
import { useConnection } from '../store/connection';
import { useUi } from '../store/ui';
import { useWorkspace } from '../store/workspace';
import type { UiLanguage } from '../types';

/**
 * The login screen, opened from the header's sign-in button (see
 * `App.tsx`'s `LoginOverlay`) rather than gating the app — a guest browses
 * the backend's own shared seed content (see `store/workspace`'s
 * `offlineData`) until they choose to authenticate; there's no account to
 * show them without one. The language menu here writes straight to
 * `updateSettings` since there's no session-backed workspace yet to read
 * `uiLanguage` from; under the guest fallback that's a local-only write
 * (same as everywhere else pre-login), and the already-mounted shell's own
 * effect picks the new value up immediately.
 */
export function LoginPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const backendUrl = useConnection((s) => s.backendUrl);
  const setTokens = useAuth((s) => s.setTokens);
  const uiLanguage = useWorkspace((s) => s.settings.uiLanguage);
  const updateSettings = useWorkspace((s) => s.updateSettings);
  const closeLogin = useUi((s) => s.closeLogin);
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [status, setStatus] = useState<'idle' | 'submitting' | 'error'>('idle');
  const [error, setError] = useState('');

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!backendUrl || status === 'submitting') return;
    setStatus('submitting');
    try {
      const tokens = await login(backendUrl, username, password);
      setTokens(tokens);
      closeLogin();
      navigate('/');
    } catch {
      setStatus('error');
      setError(t('auth.invalidCredentials'));
      return;
    }
    setStatus('idle');
  };

  return (
    <Center h="100dvh" p="md" style={{ backgroundColor: 'var(--mantine-color-body)' }}>
      <Paper withBorder radius="md" p="xl" w={360} pos="relative">
        <Group pos="absolute" top={12} right={12} gap={4}>
          <Menu shadow="md" width={160} position="bottom-end">
            <Menu.Target>
              <Tooltip label={t('settings.uiLanguage')}>
                <ActionIcon variant="subtle" size="lg" aria-label={t('settings.uiLanguage')}>
                  <IconLanguage size={18} />
                </ActionIcon>
              </Tooltip>
            </Menu.Target>
            <Menu.Dropdown>
              {UI_LANGUAGES.map((lang) => (
                <Menu.Item
                  key={lang.value}
                  onClick={() => updateSettings({ uiLanguage: lang.value as UiLanguage })}
                  fw={lang.value === uiLanguage ? 700 : 400}
                >
                  {lang.label}
                </Menu.Item>
              ))}
            </Menu.Dropdown>
          </Menu>
          <Tooltip label={t('common.close')}>
            <ActionIcon
              variant="subtle"
              size="lg"
              onClick={closeLogin}
              aria-label={t('common.close')}
            >
              <IconX size={18} />
            </ActionIcon>
          </Tooltip>
        </Group>

        <form onSubmit={(e) => void handleSubmit(e)}>
          <Stack gap="md">
            <Title order={3}>{t('auth.title')}</Title>

            {status === 'error' && (
              <Alert color="red" variant="light">
                {error}
              </Alert>
            )}

            <TextInput
              label={t('auth.username')}
              value={username}
              onChange={(e) => setUsername(e.currentTarget.value)}
              autoComplete="username"
              data-autofocus
              required
            />
            <PasswordInput
              label={t('auth.password')}
              value={password}
              onChange={(e) => setPassword(e.currentTarget.value)}
              autoComplete="current-password"
              required
            />

            <Button type="submit" loading={status === 'submitting'} fullWidth mt="xs">
              {status === 'submitting' ? t('auth.signingIn') : t('auth.signIn')}
            </Button>
          </Stack>
        </form>
      </Paper>
    </Center>
  );
}
