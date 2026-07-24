import { Badge, type MantineSize } from '@mantine/core';
import type { Department, Organization } from '../types';

interface Props {
  organization?: Organization;
  department?: Department;
  size?: MantineSize;
}

/**
 * Renders a persona's organization and (nested) department as a single tag, so
 * the two read as one "Org · Department" unit rather than two loose badges.
 */
export function OrgTag({ organization, department, size = 'xs' }: Props) {
  if (!organization) return null;
  return (
    <Badge size={size} variant="light" color={organization.color ?? 'gray'}>
      {department ? `${organization.name} · ${department.name}` : organization.name}
    </Badge>
  );
}
