import { Badge, Tooltip } from '@mantine/core';
import { useTranslation } from 'react-i18next';
import { useBackendStatus } from '../lib/useBackendStatus';
import { useAuth } from '../store/auth';

/**
 * Badge showing the current data source. Keeps three facts separate: whether
 * the backend is reachable (offline vs online), whether it's in mock mode —
 * no LLM, in-memory (mock/yellow) vs a live LLM backend (live/green) — and,
 * independently of both, whether *this browser* has a session for a backend
 * that's actually live and requires one (guest/blue). That last case matters
 * on its own: without it, a logged-out visitor on a real, reachable,
 * auth-requiring backend would see the green "Live" badge while actually
 * looking at the in-browser demo (see `isGuestFallback`) — confusingly
 * implying the backend itself is unreachable or mocked when it's neither.
 */
export function DataSourceBadge() {
  const { t } = useTranslation();
  const { reachable, mock, authRequired } = useBackendStatus();
  const accessToken = useAuth((s) => s.accessToken);

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
  if (reachable === 'online' && authRequired !== false && !accessToken) {
    return (
      <Tooltip label={t('badge.guestDemoHint')}>
        <Badge variant="light" color="blue" size="sm">
          {t('badge.guestDemo')}
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
