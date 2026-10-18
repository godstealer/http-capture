import {formatResponse} from './formatResponse';
self.onmessage = async (event: MessageEvent<{text: string; parser: string}>) => {
  try { self.postMessage({text: await formatResponse(event.data.text, event.data.parser)}); }
  catch (error) { self.postMessage({error: error instanceof Error ? error.message : String(error)}); }
};
