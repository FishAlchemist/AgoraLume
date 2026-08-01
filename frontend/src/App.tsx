import { AppShell, Burger, Center, Group, Loader, Text, Title } from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import { lazy, Suspense, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { HashRouter, Navigate, Route, Routes, useNavigate } from 'react-router-dom';
import { AppNav } from './components/AppNav';
import { ConfirmDialog } from './components/ConfirmDialog';
import { DataSourceBadge } from './components/DataSourceBadge';
import { HeaderControls } from './components/HeaderControls';
import { MemoryDrawer } from './components/MemoryDrawer';
import { PersonaCard } from './components/PersonaCard';
import { PersonaFormModal } from './components/PersonaFormModal';
import { refreshAccessToken } from './lib/api/authFetch';
import { LoginPage } from './pages/LoginPage';
import { useAuth } from './store/auth';
import { useUi } from './store/ui';
import { useWorkspace } from './store/workspace';

// Each page is a separate chunk, fetched on first navigation to it. The Suspense
// boundary below sits *inside* the main content area, so the shell (header +
// navbar) — the "structure" — paints immediately and only the content region
// shows a spinner while a page's chunk loads, then swaps in. Named exports are
// unwrapped to the default `lazy()` expects.
const AdminPage = lazy(() => import('./pages/AdminPage').then((m) => ({ default: m.AdminPage })));
const ChatPage = lazy(() => import('./pages/ChatPage').then((m) => ({ default: m.ChatPage })));
const MePage = lazy(() => import('./pages/MePage').then((m) => ({ default: m.MePage })));
const OrganizationsPage = lazy(() =>
  import('./pages/OrganizationsPage').then((m) => ({ default: m.OrganizationsPage })),
);
const PersonasPage = lazy(() =>
  import('./pages/PersonasPage').then((m) => ({ default: m.PersonasPage })),
);
const SettingsPage = lazy(() =>
  import('./pages/SettingsPage').then((m) => ({ default: m.SettingsPage })),
);

const HEADER_HEIGHT = 56;

export function App() {
  return (
    <HashRouter>
      <Shell />
      <LoginOverlay />
    </HashRouter>
  );
}

/**
 * The login screen, shown over the shell rather than in place of it — a
 * connected backend that requires auth still renders the shell against the
 * in-browser demo (see `isGuestFallback`) rather than being blocked outright,
 * and logging in is something the user opts into (see the header's sign-in
 * button) instead of a wall between them and the app. The shell stays
 * mounted underneath so its `uiLanguage` effect keeps driving `i18n` even
 * while this is open.
 */
function LoginOverlay() {
  const loginOpen = useUi((s) => s.loginOpen);
  if (!loginOpen) return null;
  return (
    // Above the shell's own chrome (AppShell's header/navbar sit at Mantine's
    // `--mantine-z-index-app`, 100) but below Mantine's popover tier (300) —
    // LoginPage's own language menu and tooltips are Mantine popovers,
    // portaled to <body> at that tier, and must render above this backdrop,
    // not behind it.
    <div style={{ position: 'fixed', inset: 0, zIndex: 150 }}>
      <LoginPage />
    </div>
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

  // A session restored from localStorage is never checked against the
  // backend until something needs it — since tokens live only in the
  // backend's memory (see backend/src/auth.rs), a restart there leaves a
  // stale-but-persisted token looking logged-in in the header until the
  // first authorized request gets rejected. Verify it once up front instead.
  useEffect(() => {
    const { refreshToken, clear } = useAuth.getState();
    if (!refreshToken) return;
    void refreshAccessToken().then((ok) => {
      if (!ok) clear();
    });
  }, []);

  return (
    <AppShell
      header={{ height: HEADER_HEIGHT }}
      navbar={{ width: 280, breakpoint: 'sm', collapsed: { mobile: !opened } }}
      // Snappier than the 200ms default — the mobile navbar should read as an
      // instant open/close, not a drawer that eases in.
      transitionDuration={150}
      padding={0}
    >
      <AppShell.Header>
        <Group h="100%" px="md" justify="space-between">
          <Group gap="sm" wrap="nowrap">
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
        <div style={{ height: 'calc(100dvh - var(--app-shell-header-height))', overflowY: 'auto' }}>
          <Suspense fallback={<PageLoader />}>
            <Routes>
              <Route path="/" element={<IndexRedirect />} />
              <Route path="/g/:groupId" element={<ChatPage />} />
              <Route path="/personas" element={<PersonasPage />} />
              <Route path="/organizations" element={<OrganizationsPage />} />
              <Route path="/me" element={<MePage />} />
              <Route path="/settings" element={<SettingsPage />} />
              <Route path="/admin" element={<AdminPage />} />
              <Route path="*" element={<Navigate to="/" replace />} />
            </Routes>
          </Suspense>
        </div>
      </AppShell.Main>

      <PersonaCard />
      <MemoryDrawer />
      <PersonaEditorHost />
      <ConfirmDialog />
    </AppShell>
  );
}

/** Fills the content area with a spinner while a lazy page chunk is loading. */
function PageLoader() {
  return (
    <Center h="100%">
      <Loader />
    </Center>
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
