import { invoke } from "./api";
import type { Flow } from "./types";

export async function importSession(
  file: File,
  progress: (count: number, total: number) => void,
) {
  const flows = await new Promise<Flow[]>((resolve, reject) => {
    const worker = new Worker(
      new URL("./session-import.worker.ts", import.meta.url),
      { type: "module" },
    );
    worker.onmessage = (event) => {
      worker.terminate();
      event.data.error
        ? reject(new Error(event.data.error))
        : resolve(event.data.flows);
    };
    worker.onerror = () => {
      worker.terminate();
      reject(new Error("Unable to parse session file"));
    };
    worker.postMessage(file);
  });
  const id = await invoke<string>("begin_import");
  let done = 0;
  try {
    let batch: Flow[] = [],
      size = 0;
    const flush = async () => {
      if (!batch.length) return;
      done = await invoke<number>("append_import", {
        id,
        offset: done,
        flows: batch,
      });
      progress(done, flows.length);
      batch = [];
      size = 0;
    };
    for (const flow of flows) {
      const length = new TextEncoder().encode(JSON.stringify(flow)).length;
      if (
        batch.length &&
        (size + length > 32 * 1024 * 1024 || batch.length >= 1000)
      )
        await flush();
      if (length > 60 * 1024 * 1024)
        throw new Error(
          "A single record exceeds the 60 MiB import transport limit",
        );
      batch.push(flow);
      size += length;
    }
    await flush();
    return await invoke<number>("finish_import", { id, commit: true });
  } catch (error) {
    await invoke("finish_import", { id, commit: false }).catch(() => {});
    throw error;
  }
}
