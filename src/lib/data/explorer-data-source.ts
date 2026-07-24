import type {
  BreadcrumbSegment,
  DirectoryRef,
  FileEntrySummary,
  FileOperationBatchResult,
  FileMoveResult,
  FileOperationPrompt,
  FileOperationPromptResponse,
  FileRemovalResult,
  ImagePreviewMode,
  LocationSummary,
  ManualSshTargetInput,
  NativeOpenOutcome,
  NativeOpenProgress,
  PreviewSummary,
  SshConnectionEvent,
  SshPromptResponse,
  SshTargetSummary,
  VolumeSnapshot,
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

export interface ConnectSshOptions {
  signal: AbortSignal;
  onEvent: (
    event: SshConnectionEvent,
    respond: (response: SshPromptResponse) => Promise<void>,
  ) => void;
}

export interface PreparedPreview {
  preview: PreviewSummary;
  dispose: () => void;
}

export interface PreparePreviewOptions {
  signal: AbortSignal;
  imageMode: ImagePreviewMode;
}

export interface OpenEntryOptions {
  signal: AbortSignal;
  allowLargeRemoteDownload: boolean;
  onProgress: (progress: NativeOpenProgress) => void;
}

export interface WatchVolumesOptions {
  signal: AbortSignal;
  onSnapshot: (snapshot: VolumeSnapshot) => void;
}

export interface FileOperationOptions {
  signal: AbortSignal;
  onProgress?: (progress: FileOperationProgress) => void;
  onPrompt: (
    prompt: FileOperationPrompt,
    respond: (response: FileOperationPromptResponse) => Promise<void>,
  ) => void;
}

export interface FileOperationProgress {
  completedItems: number;
  totalItems: number;
  completedBytes: string | null;
  totalBytes: string | null;
  currentItemCompleted: number | null;
  currentItemTotal: number | null;
}

export type RemoveEntryOptions = FileOperationOptions;

export interface ExplorerDataSource {
  getNativeOpenStartupWarning(signal: AbortSignal): Promise<string | null>;
  listLocations(signal: AbortSignal): Promise<readonly LocationSummary[]>;
  watchVolumes(options: WatchVolumesOptions): Promise<void>;
  listSshTargets(signal: AbortSignal): Promise<readonly SshTargetSummary[]>;
  createSshTarget(
    input: ManualSshTargetInput,
    signal: AbortSignal,
  ): Promise<SshTargetSummary>;
  updateSshTarget(
    targetId: string,
    input: ManualSshTargetInput,
    signal: AbortSignal,
  ): Promise<SshTargetSummary>;
  deleteSshTarget(targetId: string, signal: AbortSignal): Promise<void>;
  connectSshTarget(
    targetId: string,
    options: ConnectSshOptions,
  ): Promise<LocationSummary>;
  disconnectSshTarget(targetId: string, signal: AbortSignal): Promise<void>;
  listDirectory(
    directory: DirectoryRef,
    options: ListDirectoryOptions,
  ): Promise<void>;
  renameEntry(
    entry: FileEntrySummary,
    newName: string,
    signal: AbortSignal,
  ): Promise<FileEntrySummary>;
  moveEntry(
    entry: FileEntrySummary,
    destination: DirectoryRef,
    options: FileOperationOptions,
  ): Promise<FileMoveResult>;
  moveEntries(
    entries: readonly FileEntrySummary[],
    destination: DirectoryRef,
    options: FileOperationOptions,
  ): Promise<FileOperationBatchResult>;
  trashEntry(
    entry: FileEntrySummary,
    options: RemoveEntryOptions,
  ): Promise<FileRemovalResult>;
  trashEntries(
    entries: readonly FileEntrySummary[],
    options: RemoveEntryOptions,
  ): Promise<FileOperationBatchResult>;
  deleteEntryPermanently(
    entry: FileEntrySummary,
    options: RemoveEntryOptions,
  ): Promise<FileRemovalResult>;
  deleteEntriesPermanently(
    entries: readonly FileEntrySummary[],
    options: RemoveEntryOptions,
  ): Promise<FileOperationBatchResult>;
  getPreview(
    entry: FileEntrySummary,
    options: PreparePreviewOptions,
  ): Promise<PreparedPreview>;
  openEntry(
    entry: FileEntrySummary,
    options: OpenEntryOptions,
  ): Promise<NativeOpenOutcome>;
}
