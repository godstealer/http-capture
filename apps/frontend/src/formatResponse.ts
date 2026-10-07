import { format } from 'prettier/standalone';
import type { Plugin } from 'prettier';
import * as babel from 'prettier/plugins/babel';
import * as estree from 'prettier/plugins/estree';
import * as html from 'prettier/plugins/html';
import * as postcss from 'prettier/plugins/postcss';

export function responseLanguage(contentType: string): string {
  const mime = contentType.split(';')[0].trim().toLowerCase();
  if (mime === 'application/json' || mime.endsWith('+json')) return 'json';
  if (['text/html', 'application/xhtml+xml'].includes(mime)) return 'html';
  if (/(?:java|ecma)script$/.test(mime)) return 'babel';
  if (mime === 'text/css') return 'css';
  return 'text';
}

export async function formatResponse(text: string, parser: string): Promise<string> {
  if (parser === 'text') return text;
  if (text.length > 1024 * 1024) throw new Error('正文超过 1 MiB，显示原文以避免格式化耗时过长');
  // Prettier 3.6 ships an empty declaration for its runtime estree printer.
  return format(text, { parser: parser === 'json' ? 'json-stringify' : parser, plugins: [babel, estree as Plugin, html, postcss], tabWidth: 2, printWidth: 100 });
}
