import {
  ActionIcon,
  Alert,
  Box,
  Button,
  Card,
  Group,
  PasswordInput,
  Stack,
  Text,
  TextInput,
  Title,
} from '@mantine/core';
import { IconPencil } from '@tabler/icons-react';
import { type FormEvent, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { createAccount, listAccounts, updateAccount } from '../lib/api/accounts';
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

  const handleAccountUpdated = (updated: AccountSummary) => {
    setAccounts((prev) => prev.map((a) => (a.accountId === updated.accountId ? updated : a)));
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
              <Stack gap="xs">
                {accounts.map((a) => (
                  <AccountRow
                    key={a.accountId}
                    account={a}
                    backendUrl={backendUrl}
                    onUpdated={handleAccountUpdated}
                  />
                ))}
              </Stack>
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

/**
 * One account in the list, togglable between a plain readout and an inline
 * edit form (username + an optional new password — blank keeps the current
 * one, same "leave it out to not change it" contract as `updateAccount`'s
 * patch). Its own row-local state, not lifted into `AdminPage`, since only
 * one row is ever mid-edit and nothing else needs to know that while it's
 * happening.
 */
function AccountRow({
  account,
  backendUrl,
  onUpdated,
}: {
  account: AccountSummary;
  backendUrl: string;
  onUpdated: (updated: AccountSummary) => void;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [username, setUsername] = useState(account.username);
  const [password, setPassword] = useState('');
  const [status, setStatus] = useState<'idle' | 'submitting' | 'error'>('idle');
  const [error, setError] = useState('');

  const startEdit = () => {
    setUsername(account.username);
    setPassword('');
    setStatus('idle');
    setError('');
    setEditing(true);
  };

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (status === 'submitting') return;
    setStatus('submitting');
    setError('');
    try {
      const updated = await updateAccount(backendUrl, account.accountId, {
        username,
        ...(password && { password }),
      });
      onUpdated(updated);
      setEditing(false);
    } catch (e) {
      setStatus('error');
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  if (!editing) {
    return (
      <Group justify="space-between" wrap="nowrap">
        <Text size="sm">{account.username}</Text>
        <ActionIcon size="sm" variant="subtle" onClick={startEdit} aria-label={t('common.edit')}>
          <IconPencil size={14} />
        </ActionIcon>
      </Group>
    );
  }

  return (
    <form onSubmit={(e) => void handleSubmit(e)}>
      <Stack gap="xs">
        {status === 'error' && (
          <Alert color="red" variant="light" py={4}>
            <Text size="xs">{error}</Text>
          </Alert>
        )}
        <TextInput
          size="xs"
          label={t('auth.username')}
          value={username}
          onChange={(e) => setUsername(e.currentTarget.value)}
          autoComplete="off"
          required
        />
        <PasswordInput
          size="xs"
          label={t('auth.password')}
          description={t('admin.newPasswordHint')}
          value={password}
          onChange={(e) => setPassword(e.currentTarget.value)}
          autoComplete="new-password"
        />
        <Group justify="flex-end" gap="xs">
          <Button
            size="xs"
            variant="subtle"
            onClick={() => setEditing(false)}
            disabled={status === 'submitting'}
          >
            {t('common.cancel')}
          </Button>
          <Button size="xs" type="submit" loading={status === 'submitting'}>
            {t('common.save')}
          </Button>
        </Group>
      </Stack>
    </form>
  );
}
