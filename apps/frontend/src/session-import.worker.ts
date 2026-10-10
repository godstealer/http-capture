import { readSession } from "./sessions";
self.onmessage = async (event: MessageEvent<File>) => {
  try {
    self.postMessage({ flows: readSession(await event.data.text()) });
  } catch (error) {
    self.postMessage({ error: String(error) });
  }
};
