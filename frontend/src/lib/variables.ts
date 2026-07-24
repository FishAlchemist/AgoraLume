let counter = 0;
/** A process-local id, used only as a stable React key for editor rows. */
const rowId = () => `var-${counter++}`;

export interface VarEntry {
  /** Stable identity for React keys; not persisted. */
  id: string;
  key: string;
  value: string;
}

/** Creates a blank editor row. */
export function blankEntry(): VarEntry {
  return { id: rowId(), key: '', value: '' };
}

/** Converts editor entries to a variable record, dropping blank keys. */
export function entriesToRecord(entries: VarEntry[]): Record<string, string> {
  const record: Record<string, string> = {};
  for (const { key, value } of entries) {
    const k = key.trim();
    if (k) record[k] = value;
  }
  return record;
}

/** Converts a variable record to editor entries. */
export function recordToEntries(record: Record<string, string> | undefined): VarEntry[] {
  return Object.entries(record ?? {}).map(([key, value]) => ({ id: rowId(), key, value }));
}
