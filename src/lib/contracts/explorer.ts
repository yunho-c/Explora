export type LocationKind = "local" | "volume" | "ssh";
export type LocationStatus = "available" | "connected" | "offline";
export type EntryKind = "directory" | "file";
export type ContentKind =
  | "folder"
  | "image"
  | "document"
  | "code"
  | "audio"
  | "video"
  | "archive"
  | "other";
export type ViewMode = "list" | "grid";
export type SortColumn = "name" | "modifiedAt" | "size";
export type SortDirection = "ascending" | "descending";

export interface LocationSummary {
  id: string;
  name: string;
  kind: LocationKind;
  status: LocationStatus;
  displayPath: string;
  detail: string;
}

export interface FileEntrySummary {
  id: string;
  locationId: string;
  name: string;
  kind: EntryKind;
  contentKind: ContentKind;
  size: number | null;
  modifiedAt: string;
  detail?: string;
}

export interface ExplorerTab {
  id: string;
  title: string;
  locationId: string;
  history: string[];
  historyIndex: number;
}

export interface SortDescriptor {
  column: SortColumn;
  direction: SortDirection;
}

export interface PreviewDetail {
  label: string;
  value: string;
}

export interface PreviewSummary {
  entryId: string;
  kind: ContentKind;
  title: string;
  subtitle: string;
  excerpt?: string;
  details: PreviewDetail[];
}
