import type {
  BreadcrumbSegment,
  DirectoryRef,
  FileEntrySummary,
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
  getPreview(
    entry: FileEntrySummary,
    options: PreparePreviewOptions,
  ): Promise<PreparedPreview>;
  openEntry(
    entry: FileEntrySummary,
    options: OpenEntryOptions,
  ): Promise<NativeOpenOutcome>;
}
