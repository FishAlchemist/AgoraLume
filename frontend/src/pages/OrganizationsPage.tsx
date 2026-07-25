import {
  ActionIcon,
  Badge,
  Box,
  Button,
  Card,
  ColorSwatch,
  Divider,
  Group,
  SimpleGrid,
  Stack,
  Text,
  Title,
  Tooltip,
} from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import { IconPencil, IconPlus, IconTrash } from '@tabler/icons-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { DepartmentFormModal } from '../components/DepartmentFormModal';
import { OrganizationFormModal } from '../components/OrganizationFormModal';
import { useReadOnly } from '../store/readonly';
import { useUi } from '../store/ui';
import { useWorkspace } from '../store/workspace';
import type { Department, Organization } from '../types';

interface DeptModalState {
  organizationId: string;
  department?: Department;
}

export function OrganizationsPage() {
  const { t } = useTranslation();
  const organizations = useWorkspace((s) => s.organizations);
  const departments = useWorkspace((s) => s.departments);
  const personas = useWorkspace((s) => s.personas);
  const deleteOrganization = useWorkspace((s) => s.deleteOrganization);
  const deleteDepartment = useWorkspace((s) => s.deleteDepartment);
  const readOnly = useReadOnly((s) => s.readOnly);
  const askConfirm = useUi((s) => s.askConfirm);

  const [orgOpened, orgHandlers] = useDisclosure(false);
  const [editingOrg, setEditingOrg] = useState<Organization | undefined>(undefined);
  const [deptModal, setDeptModal] = useState<DeptModalState | null>(null);

  const openCreateOrg = () => {
    setEditingOrg(undefined);
    orgHandlers.open();
  };
  const openEditOrg = (org: Organization) => {
    setEditingOrg(org);
    orgHandlers.open();
  };
  const handleDeleteOrg = (org: Organization) => {
    askConfirm({
      message: t('organizations.confirmDelete'),
      confirmLabel: t('common.delete'),
      danger: true,
      onConfirm: () => deleteOrganization(org.id),
    });
  };
  const handleDeleteDept = (dept: Department) => {
    askConfirm({
      message: t('departments.confirmDelete'),
      confirmLabel: t('common.delete'),
      danger: true,
      onConfirm: () => deleteDepartment(dept.id),
    });
  };

  return (
    <Box p="lg">
      <Group justify="space-between" align="flex-end" mb="lg">
        <Stack gap={2}>
          <Title order={3}>{t('organizations.title')}</Title>
          <Text c="dimmed" size="sm">
            {t('organizations.subtitle')}
          </Text>
        </Stack>
        {!readOnly && (
          <Button leftSection={<IconPlus size={16} />} onClick={openCreateOrg}>
            {t('organizations.add')}
          </Button>
        )}
      </Group>

      {organizations.length === 0 ? (
        <Text c="dimmed">{t('organizations.empty')}</Text>
      ) : (
        <SimpleGrid cols={{ base: 1, md: 2 }} spacing="md">
          {organizations.map((org) => {
            const orgDepartments = departments.filter((d) => d.organizationId === org.id);
            const memberCount = personas.filter((p) => p.organizationId === org.id).length;
            const varCount = Object.keys(org.variables ?? {}).length;
            return (
              <Card key={org.id} withBorder radius="lg" padding="md">
                <Group justify="space-between" wrap="nowrap" align="flex-start">
                  <Group wrap="nowrap" miw={0}>
                    <ColorSwatch
                      color={`var(--mantine-color-${org.color ?? 'gray'}-6)`}
                      size={22}
                    />
                    <Text fw={700} truncate>
                      {org.name}
                    </Text>
                  </Group>
                  {!readOnly && (
                    <Group gap={2} wrap="nowrap">
                      <Tooltip label={t('common.edit')}>
                        <ActionIcon
                          variant="subtle"
                          onClick={() => openEditOrg(org)}
                          aria-label={t('common.edit')}
                        >
                          <IconPencil size={16} />
                        </ActionIcon>
                      </Tooltip>
                      <Tooltip label={t('common.delete')}>
                        <ActionIcon
                          variant="subtle"
                          color="red"
                          onClick={() => handleDeleteOrg(org)}
                          aria-label={t('common.delete')}
                        >
                          <IconTrash size={16} />
                        </ActionIcon>
                      </Tooltip>
                    </Group>
                  )}
                </Group>

                {org.blurb && (
                  <Text size="sm" c="dimmed" mt="sm" lineClamp={2}>
                    {org.blurb}
                  </Text>
                )}
                <Group gap="xs" mt="sm">
                  <Badge variant="light" color={org.color ?? 'gray'}>
                    {t('organizations.memberCount', { count: memberCount })}
                  </Badge>
                  {varCount > 0 && (
                    <Badge variant="outline" color="gray">
                      {t('organizations.variables')}: {varCount}
                    </Badge>
                  )}
                </Group>

                <Divider my="sm" />

                <Group justify="space-between" mb="xs">
                  <Text size="xs" c="dimmed" tt="uppercase" fw={700}>
                    {t('departments.title')}
                  </Text>
                  {!readOnly && (
                    <Button
                      size="compact-xs"
                      variant="light"
                      leftSection={<IconPlus size={12} />}
                      onClick={() => setDeptModal({ organizationId: org.id })}
                    >
                      {t('departments.add')}
                    </Button>
                  )}
                </Group>

                {orgDepartments.length === 0 ? (
                  <Text size="xs" c="dimmed">
                    {t('departments.empty')}
                  </Text>
                ) : (
                  <Stack gap={4}>
                    {orgDepartments.map((dept) => {
                      const deptMembers = personas.filter((p) => p.departmentId === dept.id).length;
                      return (
                        <Group key={dept.id} justify="space-between" wrap="nowrap" gap="xs">
                          <Group gap="xs" wrap="nowrap" miw={0}>
                            <ColorSwatch
                              color={`var(--mantine-color-${dept.color ?? 'gray'}-6)`}
                              size={14}
                            />
                            <Text size="sm" truncate>
                              {dept.name}
                            </Text>
                            <Text size="xs" c="dimmed">
                              · {t('common.members', { count: deptMembers })}
                            </Text>
                          </Group>
                          {!readOnly && (
                            <Group gap={0} wrap="nowrap">
                              <ActionIcon
                                size="sm"
                                variant="subtle"
                                onClick={() =>
                                  setDeptModal({ organizationId: org.id, department: dept })
                                }
                                aria-label={t('common.edit')}
                              >
                                <IconPencil size={14} />
                              </ActionIcon>
                              <ActionIcon
                                size="sm"
                                variant="subtle"
                                color="red"
                                onClick={() => handleDeleteDept(dept)}
                                aria-label={t('common.delete')}
                              >
                                <IconTrash size={14} />
                              </ActionIcon>
                            </Group>
                          )}
                        </Group>
                      );
                    })}
                  </Stack>
                )}
              </Card>
            );
          })}
        </SimpleGrid>
      )}

      <OrganizationFormModal
        opened={orgOpened}
        onClose={orgHandlers.close}
        organization={editingOrg}
      />
      <DepartmentFormModal
        opened={deptModal !== null}
        onClose={() => setDeptModal(null)}
        organizationId={deptModal?.organizationId ?? ''}
        department={deptModal?.department}
      />
    </Box>
  );
}
