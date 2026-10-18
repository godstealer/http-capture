export interface ExportRequest {
  method: string;
  url: string;
  headers: { name: string; value: string }[];
  bodyBase64: string;
  bodyBytes: Uint8Array;
  bodyText: string | null;
}

export interface CodeTarget {
  id: string;
  language: string;
  library: string;
  filename: string;
  parser?: string;
  browser?: boolean;
  preservesDuplicates?: boolean;
  setup?: string;
  generate: (request: ExportRequest) => string;
}

// SDKs that require a dictionary cannot represent duplicate field lines. Their
// targets are flagged so the export dialog can explain the conversion.
export function mergedHeaders(request: ExportRequest): { name: string; value: string }[] {
  const rows: { name: string; value: string }[] = [];
  const indexes = new Map<string, number>();
  for (const header of request.headers) {
    const lower = header.name.toLowerCase();
    const index = indexes.get(lower);
    if (index === undefined) { indexes.set(lower, rows.length); rows.push({ ...header }); }
    else rows[index].value += (lower === 'cookie' ? '; ' : ', ') + header.value;
  }
  return rows;
}
