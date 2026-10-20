import type { Flow } from "./types";
export interface FlowChanges<T extends {id: string} = Flow> {
  revision: string;
  reset: boolean;
  rows: { position: number; flow: T }[];
  deleted: string[];
}
export function createFlowChanges<T extends {id: string} = Flow>(
  fetch: (since: string | null) => Promise<FlowChanges<T>>,
) {
  let revision: string | null = null,
    epoch = -1;
  let rows = new Map<string, { position: number; flow: T }>();
  let attempt = 0;
  return async (nextEpoch: number, apply: (flows: T[]) => void) => {
    const ticket = ++attempt;
    const fresh = epoch !== nextEpoch || revision === null;
    const changes = await fetch(fresh ? null : revision);
    if (ticket !== attempt) return;
    const updated =
      fresh || changes.reset || changes.rows.length > 0 || changes.deleted.length > 0;
    if (updated) {
      const next = fresh || changes.reset
        ? new Map<string, { position: number; flow: T }>()
        : new Map(rows);
      changes.deleted.forEach((id) => next.delete(id));
      changes.rows.forEach((row) => next.set(row.flow.id, row));
      apply(
        [...next.values()]
          .sort((a, b) => b.position - a.position)
          .map((row) => row.flow),
      );
      rows = next;
    }
    revision = changes.revision;
    epoch = nextEpoch;
  };
}
