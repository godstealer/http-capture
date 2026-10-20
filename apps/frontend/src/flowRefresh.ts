// Read the revision BEFORE the snapshot: a concurrent write must trigger the next poll.
// Commit it only after the caller has applied the snapshot successfully.
export function isMissingFlowEndpoint(error: unknown, command: string): boolean {
  const message = String(error).replace(/^Error:\s*/, '').trim();
  if (/^HTTP 404(?:\s*:|$)/.test(message)) return true;
  return message === `Unknown command: ${command}` || message === `Command ${command} not found` || message === `Command \"${command}\" not found`;
}
export function createFlowRefresh<T>(revision: () => Promise<string>, list: () => Promise<T[]>) {
  let applied: string | undefined;
  let attempt = 0;
  return async (apply: (rows: T[]) => void) => {
    const ticket = ++attempt;
    let current: string | undefined;
    try { current = await revision(); }
    catch (error) { if (!isMissingFlowEndpoint(error, 'flow_revision')) throw error; }
    if (ticket !== attempt) return;
    if (current !== undefined && current === applied) return;
    const rows = await list();
    if (ticket !== attempt) return;
    apply(rows);
    applied = current;
  };
}
