import type { Header } from './types';

export type HeaderTextErrorCode = 'invalidLineEnding' | 'invalidLine' | 'invalidName' | 'invalidValue' | 'missingValue';

const errorDescriptions: Record<HeaderTextErrorCode, string> = {
  invalidLineEnding: 'A carriage return must be followed by a line feed',
  invalidLine: 'Expected a header in Name: value format',
  invalidName: 'Invalid HTTP header name',
  invalidValue: 'Invalid control character in HTTP header value',
  missingValue: 'Missing value for a DevTools header name',
};

export class HeaderTextError extends Error {
  constructor(public readonly code: HeaderTextErrorCode, public readonly line: number, public readonly headerName?: string) {
    super(`${errorDescriptions[code]} (line ${line})`);
    this.name = 'HeaderTextError';
  }
}

export interface ParsedHeaderText { headers: Header[]; pseudoHeaders: Header[] }

const token = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
const pseudoName = /^:[!#$%&'*+\-.^_`|~0-9a-z]+$/;
const invalidValue = /[\x00-\x08\x0a-\x1f\x7f]/;
const startLine = /^(?:[!#$%&'*+\-.^_`|~0-9A-Za-z]+[ \t]+\S+[ \t]+HTTP\/(?:1\.[01]|[23](?:\.0)?)|HTTP\/(?:1\.[01]|[23](?:\.0)?)[ \t]+[0-9]{3}(?:[ \t]+.*)?)$/;

function validateName(name: string, line: number, allowPseudo: boolean) {
  if (!token.test(name) && !(allowPseudo && pseudoName.test(name))) {
    throw new HeaderTextError('invalidName', line, name);
  }
}

function validateValue(value: string, line: number, name?: string) {
  if (invalidValue.test(value)) throw new HeaderTextError('invalidValue', line, name);
}

/** Import raw HTTP headers or the alternating name/value lines copied from DevTools.
 * Pseudo headers stay separate so pasting headers never changes the URL or method.
 */
export function parseHeaderText(text: string): ParsedHeaderText {
  const bareCarriageReturn = /\r(?!\n)/.exec(text);
  if (bareCarriageReturn) {
    throw new HeaderTextError('invalidLineEnding', text.slice(0, bareCarriageReturn.index).split('\n').length);
  }
  const result: ParsedHeaderText = { headers: [], pseudoHeaders: [] };
  if (/^[ \t\r\n]*$/.test(text)) return result;
  const lines = text.split(/\r\n|\n/);
  // A terminal newline ends the preceding line. Two newlines after a DevTools
  // name explicitly represent an empty value.
  if (lines.at(-1) === '') lines.pop();
  for (let index = 0; index < lines.length; index++) validateValue(lines[index], index + 1);
  let index = 0;
  while (lines[index] === '') index++;
  if (startLine.test(lines[index] ?? '')) index++;
  while (lines[index] === '') index++;
  const first = lines[index] ?? '';
  const devtools = token.test(first) || pseudoName.test(first);
  const append = (name: string, value: string, line: number) => {
    validateName(name, line, true);
    validateValue(value, line, name);
    (name.startsWith(':') ? result.pseudoHeaders : result.headers).push({ name, value });
  };
  while (index < lines.length) {
    const line = lines[index];
    const lineNumber = index + 1;
    if (line === '') { index++; continue; }
    if (devtools) {
      validateName(line, lineNumber, true);
      if (index + 1 >= lines.length) throw new HeaderTextError('missingValue', lineNumber, line);
      append(line, lines[index + 1], lineNumber);
      index += 2;
    } else {
      const colon = line.indexOf(':', line.startsWith(':') ? 1 : 0);
      if (colon < 0) throw new HeaderTextError('invalidLine', lineNumber);
      append(line.slice(0, colon), line.slice(colon + 1).replace(/^[ \t]+/, ''), lineNumber);
      index++;
    }
  }
  return result;
}

export function parseHeaders(text: string): Header[] {
  return parseHeaderText(text).headers;
}

/** Copy only enabled ordinary headers, preserving order, duplicates and value text. */
export function serializeHeaders(headers: readonly Header[], lineEnding: '\r\n' | '\n' = '\r\n'): string {
  if (lineEnding !== '\r\n' && lineEnding !== '\n') throw new HeaderTextError('invalidLineEnding', 1);
  const lines: string[] = [];
  headers.forEach((header, index) => {
    if (header.disabled) return;
    validateName(header.name, index + 1, false);
    validateValue(header.value, index + 1, header.name);
    lines.push(`${header.name}: ${header.value}`);
  });
  return lines.join(lineEnding);
}
