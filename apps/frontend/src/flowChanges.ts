import type { Flow } from "./types";
export interface FlowChanges {
  revision: string;
  reset: boolean;
  rows: { position: number; flow: Flow }[];
  deleted: string[];
}
export function createFlowChanges(
  fetch: (since: string | null) => Promise<FlowChanges>,
) {
  let revision: string | null = null,
    epoch = -1;
  let rows = new Map<string, { position: number; flow: Flow }>();
  return async (nextEpoch: number, apply: (flows: Flow[]) => void) => {
    const changes = await fetch(epoch === nextEpoch ? revision : null);
    const updated =
      changes.reset || changes.rows.length > 0 || changes.deleted.length > 0;
    if (updated) {
      const next = changes.reset
        ? new Map<string, { position: number; flow: Flow }>()
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
