import { AppShell, Badge, Burger, Group, Text, Title, Tooltip } from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { HashRouter, Navigate, Route, Routes, useNavigate } from 'react-router-dom';
import { AppNav } from './components/AppNav';
import { ConfirmDialog } from './components/ConfirmDialog';
import { HeaderControls } from './components/HeaderControls';
import { PersonaCard } from './components/PersonaCard';
import { PersonaFormModal } from './components/PersonaFormModal';
import { useBackendStatus } from './lib/useBackendStatus';
import { ChatPage } from './pages/ChatPage';
import { OrganizationsPage } from './pages/OrganizationsPage';
import { PersonasPage } from './pages/PersonasPage';
import { SettingsPage } from './pages/SettingsPage';
import { useUi } from './store/ui';
import { useWorkspace } from './store/workspace';

const HEADER_HEIGHT = 56;

export function App() {
  return (
    <HashRouter>
      <Shell />
    </HashRouter>
  );
}

function Shell() {
  const { i18n } = useTranslation();
  const [opened, { toggle, close }] = useDisclosure();
  const uiLanguage = useWorkspace((s) => s.settings.uiLanguage);
  const navigate = useNavigate();

  // The workspace store is the source of truth for the UI language.
  useEffect(() => {
    void i18n.changeLanguage(uiLanguage);
    document.documentElement.lang = uiLanguage;
  }, [uiLanguage, i18n]);

  return (
    <AppShell
      header={{ height: HEADER_HEIGHT }}
      navbar={{ width: 280, breakpoint: 'sm', collapsed: { mobile: !opened } }}
      padding={0}
    >
      <AppShell.Header>
        <Group h="100%" px="md" justify="space-between">
          <Group gap="sm">
            <Burger opened={opened} onClick={toggle} hiddenFrom="sm" size="sm" />
            <Title order={4} style={{ cursor: 'pointer' }} onClick={() => navigate('/')}>
              <Text
                span
                inherit
                fw={900}
                variant="gradient"
                gradient={{ from: 'indigo', to: 'cyan', deg: 120 }}
              >
                AgoraLume
              </Text>
            </Title>
            <DataSourceBadge />
          </Group>
          <HeaderControls />
        </Group>
      </AppShell.Header>

      <AppShell.Navbar p="md">
        <AppNav onNavigate={close} />
      </AppShell.Navbar>

      <AppShell.Main>
        <div style={{ height: `calc(100dvh - ${HEADER_HEIGHT}px)`, overflowY: 'auto' }}>
          <Routes>
            <Route path="/" element={<IndexRedirect />} />
            <Route path="/g/:groupId" element={<ChatPage />} />
            <Route path="/personas" element={<PersonasPage />} />
            <Route path="/organizations" element={<OrganizationsPage />} />
            <Route path="/settings" element={<SettingsPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </div>
      </AppShell.Main>

      <PersonaCard />
      <PersonaEditorHost />
      <ConfirmDialog />
    </AppShell>
  );
}

/**
 * Header badge showing the data source. Keeps two facts separate: is the
 * backend reachable (offline vs online), and is it in mock mode — no LLM,
 * in-memory (mock/yellow) vs a live LLM backend (live/green).
 */
function DataSourceBadge() {
  const { t } = useTranslation();
  const { reachable, mock } = useBackendStatus();

  if (reachable === 'checking') {
    return (
      <Badge variant="light" color="gray" size="sm">
        {t('badge.checking')}
      </Badge>
    );
  }
  if (reachable === 'offline') {
    return (
      <Tooltip label={t('badge.offlineHint')}>
        <Badge variant="light" color="red" size="sm">
          {t('badge.offline')}
        </Badge>
      </Tooltip>
    );
  }
  if (mock) {
    return (
      <Tooltip label={t('badge.mockHint')}>
        <Badge variant="light" color="yellow" size="sm">
          {reachable === 'local' ? t('badge.mockLocal') : t('badge.mock')}
        </Badge>
      </Tooltip>
    );
  }
  return (
    <Tooltip label={t('badge.liveHint')}>
      <Badge variant="light" color="green" size="sm">
        {t('badge.live')}
      </Badge>
    </Tooltip>
  );
}

/** Renders the persona editor driven by the shared UI store. */
function PersonaEditorHost() {
  const editorOpen = useUi((s) => s.editorOpen);
  const editorPersonaId = useUi((s) => s.editorPersonaId);
  const editorKind = useUi((s) => s.editorKind);
  const closeEditor = useUi((s) => s.closeEditor);
  const persona = useWorkspace((s) =>
    editorPersonaId ? s.personas.find((p) => p.id === editorPersonaId) : undefined,
  );
  return (
    <PersonaFormModal
      opened={editorOpen}
      onClose={closeEditor}
      persona={persona}
      defaultKind={editorKind}
    />
  );
}

/** Sends the bare "/" route to the first group, or shows the empty chat state. */
function IndexRedirect() {
  const firstGroupId = useWorkspace((s) => s.groups[0]?.id);
  return firstGroupId ? <Navigate to={`/g/${firstGroupId}`} replace /> : <ChatPage />;
}
