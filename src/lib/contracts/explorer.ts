export type LocationKind = "local" | "volume" | "ssh";
export type LocationRole =
  | "home"
  | "desktop"
  | "documents"
  | "downloads"
  | "pictures"
  | "music"
  | "videos"
  | "volume"
  | "ssh";
export type LocationStatus = "available" | "connected" | "offline";
export type EntryKind = "directory" | "file" | "symlink" | "other";
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

export interface EntryRef {
  id: string;
  locationId: string;
}

export interface DirectoryRef extends EntryRef {
  name: string;
  displayPath: string;
}

export interface BreadcrumbSegment {
  label: string;
  directory: DirectoryRef;
}

export interface LocationSummary {
  id: string;
  name: string;
  kind: LocationKind;
  role: LocationRole;
  status: LocationStatus;
  displayPath: string;
  detail: string;
  root: DirectoryRef;
}

export interface FileEntrySummary {
  reference: EntryRef;
  name: string;
  kind: EntryKind;
  contentKind: ContentKind;
  size: string | null;
  modifiedAt: number | null;
  displayPath: string;
  directory: DirectoryRef | null;
  detail?: string;
}

export interface ExplorerTab {
  id: string;
  title: string;
  locationId: string;
  directory: DirectoryRef;
  history: DirectoryRef[];
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
