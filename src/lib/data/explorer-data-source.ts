import type {
  BreadcrumbSegment,
  DirectoryRef,
  FileEntrySummary,
  FileMoveResult,
  FileOperationPrompt,
  FileOperationPromptResponse,
  FileRemovalResult,
  ImagePreviewMode,
  LocationSummary,
  ManualSshTargetInput,
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

export interface WatchVolumesOptions {
  signal: AbortSignal;
  onSnapshot: (snapshot: VolumeSnapshot) => void;
}

export interface FileOperationOptions {
  signal: AbortSignal;
  onPrompt: (
    prompt: FileOperationPrompt,
    respond: (response: FileOperationPromptResponse) => Promise<void>,
  ) => void;
}

export type RemoveEntryOptions = FileOperationOptions;

export interface ExplorerDataSource {
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
  trashEntry(
    entry: FileEntrySummary,
    options: RemoveEntryOptions,
  ): Promise<FileRemovalResult>;
  deleteEntryPermanently(
    entry: FileEntrySummary,
    options: RemoveEntryOptions,
  ): Promise<FileRemovalResult>;
  getPreview(
    entry: FileEntrySummary,
    options: PreparePreviewOptions,
  ): Promise<PreparedPreview>;
}
