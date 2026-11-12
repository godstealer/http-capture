import type { RequestDraft } from './types';

export type ExternalImportFormat = 'postman' | 'hoppscotch' | 'openapi' | 'insomnia' | 'har' | 'curl';

export interface ImportedRequest {
  name: string;
  draft: RequestDraft;
}

export interface ExternalImportPlan {
  source: string;
  collections: { name: string; requests: ImportedRequest[] }[];
  environments?: { name: string; variables: Record<string, string> }[];
  warnings: string[];
}
