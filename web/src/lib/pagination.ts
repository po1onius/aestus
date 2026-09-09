import { dashboardListPageSize } from "../config";
export function initialListPageState(): ListPageState {
  return { offset: 0, nextOffset: null };
}

export function pageStateFrom(page: { offset: number; next_offset: number | null }): ListPageState {
  return { offset: page.offset, nextOffset: page.next_offset };
}

export function listPagePath(path: string, offset: number) {
  const params = new URLSearchParams({
    limit: String(dashboardListPageSize),
    offset: String(offset),
  });
  return `${path}?${params.toString()}`;
}

export interface ListPageState {
  offset: number;
  nextOffset: number | null;
}
