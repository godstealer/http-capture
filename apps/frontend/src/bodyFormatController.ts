export type BodyFormatSnapshot = {
  text: string;
  parser: string;
  revision: unknown;
  readOnly: boolean;
  autoFormatOnPaste: boolean;
};

export type BodyFormatTask = { promise: Promise<string>; cancel: () => void };
export type BodyFormatStatus = {
  kind: 'idle' | 'formatting' | 'formatted' | 'error' | 'too-large';
  message?: string;
};

export const BODY_FORMAT_LIMIT = 1024 * 1024;

type Options = {
  read: () => BodyFormatSnapshot;
  start: (text: string, parser: string) => BodyFormatTask;
  apply: (text: string) => void;
  status: (state: BodyFormatStatus) => void;
};
type Outcome = { kind: 'result'; text: string } | { kind: 'error'; error: unknown } | { kind: 'cancelled' };
type Job = {
  epoch: number;
  snapshot: BodyFormatSnapshot;
  task?: BodyFormatTask;
  cancelled: Promise<Outcome>;
  resolveCancellation: (outcome: Outcome) => void;
};
const parsers = new Set(['json', 'html', 'babel', 'css']);

function matches(before: BodyFormatSnapshot, after: BodyFormatSnapshot) {
  return Object.is(before.revision, after.revision)
    && before.text === after.text
    && before.parser === after.parser
    && before.readOnly === after.readOnly
    && before.autoFormatOnPaste === after.autoFormatOnPaste;
}

/** A formatter may finish after cancellation. Only the current, unchanged
 * document can receive its result. The editor owns the resulting undo action.
 */
export function createBodyFormatController(options: Options) {
  let disposed = false;
  let epoch = 0;
  let active: Job | undefined;
  const emit = (state: BodyFormatStatus) => { if (!disposed) options.status(state); };
  const cancelTask = (task: BodyFormatTask | undefined) => {
    try { task?.cancel(); } catch { /* Cancellation must not prevent invalidation. */ }
  };
  const cancel = () => {
    epoch++;
    const job = active;
    active = undefined;
    if (job) {
      job.resolveCancellation({ kind: 'cancelled' });
      cancelTask(job.task);
    }
  };
  const current = (job: Job) => !disposed && active === job && epoch === job.epoch;

  return {
    async format(source: 'manual' | 'paste'): Promise<void> {
      if (disposed) return;
      cancel();
      // Copy the snapshot so a caller cannot mutate the comparison fields in place.
      const snapshot = { ...options.read() };
      if (snapshot.readOnly || (source === 'paste' && !snapshot.autoFormatOnPaste)
          || !parsers.has(snapshot.parser)) {
        emit({ kind: 'idle' });
        return;
      }
      if (snapshot.text.length > BODY_FORMAT_LIMIT || new TextEncoder().encode(snapshot.text).length > BODY_FORMAT_LIMIT) {
        emit({ kind: 'too-large' });
        return;
      }
      if (!snapshot.text.trim()) {
        emit({ kind: 'idle' });
        return;
      }
      let resolveCancellation!: (outcome: Outcome) => void;
      const cancelled = new Promise<Outcome>(resolve => { resolveCancellation = resolve; });
      const job: Job = { epoch, snapshot, cancelled, resolveCancellation };
      active = job;
      emit({ kind: 'formatting' });
      if (!current(job)) return;
      let outcome: Outcome;
      try {
        const task = options.start(snapshot.text, snapshot.parser);
        job.task = task;
        // Observe rejection even if cancellation wins the race or start() reenters.
        const result = task.promise.then<Outcome, Outcome>(
          text => ({ kind: 'result', text }),
          error => ({ kind: 'error', error }),
        );
        if (!current(job)) {
          cancelTask(task);
          return;
        }
        outcome = await Promise.race([result, cancelled]);
      } catch (error) {
        outcome = { kind: 'error', error };
      }
      if (!current(job)) return;
      active = undefined;
      if (outcome.kind === 'cancelled' || !matches(snapshot, options.read())) {
        emit({ kind: 'idle' });
        return;
      }
      if (outcome.kind === 'error') {
        emit({ kind: 'error', message: outcome.error instanceof Error ? outcome.error.message : String(outcome.error) });
        return;
      }
      if (typeof outcome.text !== 'string') {
        emit({ kind: 'error', message: 'Formatter returned an invalid result' });
        return;
      }
      // End busy before applying: editor change handlers may invalidate the job.
      emit({ kind: 'formatted' });
      if (!disposed && epoch === job.epoch && matches(snapshot, options.read())
          && outcome.text !== snapshot.text) {
        options.apply(outcome.text);
      }
    },
    invalidate() {
      if (disposed) return;
      cancel();
      emit({ kind: 'idle' });
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      cancel();
    },
  };
}
