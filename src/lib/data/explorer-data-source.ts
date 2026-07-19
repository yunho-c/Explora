import type {
  BreadcrumbSegment,
  DirectoryRef,
  FileEntrySummary,
  LocationSummary,
  PreviewSummary,
} from "$lib/contracts/explorer";

export interface DirectoryStart {
  directory: DirectoryRef;
  parent: DirectoryRef | null;
  breadcrumbs: readonly BreadcrumbSegment[];
}

export interface DirectoryBatch {
  entries: readonly FileEntrySummary[];
  replace: boolean;
}

export interface DirectoryComplete {
  skippedEntries: number;
}

export interface ListDirectoryOptions {
  signal: AbortSignal;
  onStart: (start: DirectoryStart) => void;
  onBatch: (batch: DirectoryBatch) => void;
  onComplete: (complete: DirectoryComplete) => void;
}

export interface ExplorerDataSource {
  listLocations(signal: AbortSignal): Promise<readonly LocationSummary[]>;
  listDirectory(
    directory: DirectoryRef,
    options: ListDirectoryOptions,
  ): Promise<void>;
  getPreview(
    entry: FileEntrySummary,
    signal: AbortSignal,
  ): Promise<PreviewSummary>;
}
