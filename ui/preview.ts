export interface PreviewEntry { path: string | null; directory: boolean; size: number }
export interface PackagePreview {
  format_version: number; total_entries: number; file_count: number; directory_count: number;
  content_bytes: number; entries: PreviewEntry[]; truncated: boolean;
}
export const PREVIEW_PAGE_SIZE = 50;

export function previewPage(preview: PackagePreview, requested: number): { page: number; pages: number; entries: PreviewEntry[] } {
  const pages = Math.max(1, Math.ceil(preview.entries.length / PREVIEW_PAGE_SIZE));
  const page = Math.min(Math.max(0, Number.isFinite(requested) ? Math.trunc(requested) : 0), pages - 1);
  return { page, pages, entries: preview.entries.slice(page * PREVIEW_PAGE_SIZE, (page + 1) * PREVIEW_PAGE_SIZE) };
}
