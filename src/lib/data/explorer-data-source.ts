import type {
  FileEntrySummary,
  LocationSummary,
  PreviewSummary,
} from "$lib/contracts/explorer";

export interface DirectoryBatch {
  entries: readonly FileEntrySummary[];
  replace: boolean;
}

export interface ListDirectoryOptions {
  signal: AbortSignal;
  onBatch: (batch: DirectoryBatch) => void;
}

export interface ExplorerDataSource {
  listLocations(signal: AbortSignal): Promise<readonly LocationSummary[]>;
  listDirectory(
    locationId: string,
    options: ListDirectoryOptions,
  ): Promise<void>;
  getPreview(
    entry: FileEntrySummary,
    signal: AbortSignal,
  ): Promise<PreviewSummary>;
}
