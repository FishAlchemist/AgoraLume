import {
  Alert,
  Box,
  Button,
  Card,
  Group,
  List,
  PasswordInput,
  Stack,
  Text,
  TextInput,
  Title,
} from '@mantine/core';
import { type FormEvent, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { createAccount, listAccounts } from '../lib/api/accounts';
import type { AccountSummary } from '../lib/api/types';
import { useConnection } from '../store/connection';

/**
 * Admin's own page: create accounts for people to log into, and see what
 * already exists. Admin has no workspace of its own (see `CurrentAccount` in
 * `backend/src/state.rs`) — this, not the chat shell, is what admin actually
 * lands on after logging in (see `LoginPage`'s post-login redirect). Doesn't
 * pre-check the session's role client-side before rendering the form: it
 * always attempts the real request and surfaces whatever the backend says,
 * same principle as `LlmProviderForm` in `SettingsPage`.
 */
export function AdminPage() {
  const { t } = useTranslation();
  const backendUrl = useConnection((s) => s.backendUrl);

  const [accounts, setAccounts] = useState<AccountSummary[]>([]);
  const [loadError, setLoadError] = useState('');

  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [status, setStatus] = useState<'idle' | 'submitting' | 'error'>('idle');
  const [error, setError] = useState('');

  useEffect(() => {
    if (!backendUrl) return;
    listAccounts(backendUrl)
      .then(setAccounts)
      .catch((e: Error) => setLoadError(e.message));
  }, [backendUrl]);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!backendUrl || status === 'submitting') return;
    setStatus('submitting');
    setError('');
    try {
      const created = await createAccount(backendUrl, username, password);
      setAccounts((prev) => [...prev, created]);
      setUsername('');
      setPassword('');
      setStatus('idle');
    } catch (e) {
      setStatus('error');
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  if (!backendUrl) {
    return (
      <Box p="lg" maw={560}>
        <Text c="dimmed">{t('settings.llmNeedsBackend')}</Text>
      </Box>
    );
  }

  return (
    <Box p="lg" maw={560}>
      <Stack gap={2} mb="lg">
        <Title order={3}>{t('admin.title')}</Title>
        <Text c="dimmed" size="sm">
          {t('admin.subtitle')}
        </Text>
      </Stack>

      <Stack gap="lg">
        <Card withBorder radius="lg" padding="lg">
          <Stack gap="sm">
            <Text fw={600} size="sm">
              {t('admin.existingAccounts')}
            </Text>
            {loadError && (
              <Alert color="red" variant="light" py={6}>
                {loadError}
              </Alert>
            )}
            {accounts.length === 0 && !loadError ? (
              <Text size="sm" c="dimmed">
                {t('admin.noAccounts')}
              </Text>
            ) : (
              <List size="sm" spacing={4}>
                {accounts.map((a) => (
                  <List.Item key={a.accountId}>{a.username}</List.Item>
                ))}
              </List>
            )}
          </Stack>
        </Card>

        <Card withBorder radius="lg" padding="lg">
          <form onSubmit={(e) => void handleSubmit(e)}>
            <Stack gap="md">
              <Text fw={600} size="sm">
                {t('admin.createAccount')}
              </Text>

              {status === 'error' && (
                <Alert color="red" variant="light" py={6}>
                  {error}
                </Alert>
              )}

              <TextInput
                label={t('auth.username')}
                value={username}
                onChange={(e) => setUsername(e.currentTarget.value)}
                autoComplete="off"
                required
              />
              <PasswordInput
                label={t('auth.password')}
                value={password}
                onChange={(e) => setPassword(e.currentTarget.value)}
                autoComplete="new-password"
                required
              />

              <Group justify="flex-end">
                <Button type="submit" loading={status === 'submitting'}>
                  {t('admin.createAccount')}
                </Button>
              </Group>
            </Stack>
          </form>
        </Card>
      </Stack>
    </Box>
  );
}
