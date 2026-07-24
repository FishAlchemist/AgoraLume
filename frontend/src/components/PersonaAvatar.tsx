import { Avatar } from '@mantine/core';
import type { Persona } from '../types';

interface Props {
  persona: Persona;
  size?: number;
  /** When provided, the avatar becomes clickable (e.g. to open the info card). */
  onClick?: () => void;
}

/**
 * Character avatar. Priority: custom image → emoji on a gradient → initials.
 */
export function PersonaAvatar({ persona, size = 42, onClick }: Props) {
  const clickable = Boolean(onClick);
  return (
    <Avatar
      src={persona.avatarUrl ?? null}
      alt={persona.name}
      radius="xl"
      size={size}
      variant="filled"
      onClick={onClick}
      role={clickable ? 'button' : undefined}
      tabIndex={clickable ? 0 : undefined}
      onKeyDown={
        clickable
          ? (event) => {
              if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                onClick?.();
              }
            }
          : undefined
      }
      style={{
        background: persona.avatarUrl
          ? undefined
          : (persona.gradient ?? 'linear-gradient(135deg, #4dabf7, #4263eb)'),
        boxShadow: '0 4px 14px rgba(0, 0, 0, 0.20)',
        fontSize: size * 0.52,
        lineHeight: 1,
        cursor: clickable ? 'pointer' : undefined,
      }}
    >
      {persona.emoji ?? persona.name.slice(0, 2).toUpperCase()}
    </Avatar>
  );
}
