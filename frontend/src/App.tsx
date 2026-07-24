import { AppShell, Badge, Burger, Group, Text, Title } from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { HashRouter, Navigate, Route, Routes, useNavigate } from 'react-router-dom';
import { AppNav } from './components/AppNav';
import { ConfirmDialog } from './components/ConfirmDialog';
import { HeaderControls } from './components/HeaderControls';
import { PersonaCard } from './components/PersonaCard';
import { PersonaFormModal } from './components/PersonaFormModal';
import { usingMock } from './lib/api';
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
  const { t, i18n } = useTranslation();
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
            {usingMock && (
              <Badge variant="light" color="yellow" size="sm">
                {t('badge.mock')}
              </Badge>
            )}
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
