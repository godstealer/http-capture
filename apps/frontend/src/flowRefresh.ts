// Read the revision BEFORE the snapshot: a concurrent write must trigger the next poll.
// Commit it only after the caller has applied the snapshot successfully.
export function createFlowRefresh<T>(revision: () => Promise<string>, list: () => Promise<T[]>) {
  let applied: string | undefined;
  return async (apply: (rows: T[]) => void) => {
    let current: string | undefined;
    try { current = await revision(); }
    catch { /* Older development services have no revision endpoint; keep full reads working. */ }
    if (current !== undefined && current === applied) return;
    const rows = await list();
    apply(rows);
    applied = current;
  };
}
