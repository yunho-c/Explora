import type {
  BreadcrumbSegment,
  DirectoryRef,
  FileEntrySummary,
  FileMoveResult,
  FileOperationPrompt,
  FileOperationPromptResponse,
  FileRemovalResult,
  LocationSummary,
} from "$lib/contracts/explorer";
import type {
  ExplorerDataSource,
  FileOperationOptions,
  FileOperationProgress,
} from "$lib/data/explorer-data-source";

interface PendingPrompt {
  prompt: FileOperationPrompt;
  responding: boolean;
  respond: (response: FileOperationPromptResponse) => Promise<void>;
}

export interface MoveChooserState {
  entry: FileEntrySummary;
  sourceParent: DirectoryRef;
  locations: readonly LocationSummary[];
  directory: DirectoryRef;
  parent: DirectoryRef | null;
  breadcrumbs: readonly BreadcrumbSegment[];
  directories: readonly FileEntrySummary[];
  loading: boolean;
  errorMessage: string | null;
}

const isAbortError = (error: unknown) =>
  error instanceof Error && error.name === "AbortError";

export class FileOperationStore {
  activeEntryId = $state<string | null>(null);
  activeEntryName = $state<string | null>(null);
  activeAction = $state<string | null>(null);
  progress = $state<FileOperationProgress | null>(null);
  pendingPrompt = $state<PendingPrompt | null>(null);
  moveChooser = $state<MoveChooserState | null>(null);
  errorMessage = $state<string | null>(null);

  private controller: AbortController | null = null;
  private chooserController: AbortController | null = null;

  constructor(private readonly dataSource: ExplorerDataSource) {}

  get canConfirmMove(): boolean {
    const chooser = this.moveChooser;
    return Boolean(
      chooser &&
      !chooser.loading &&
      chooser.directory.capabilities.acceptMove &&
      chooser.directory.locationId === chooser.entry.reference.locationId &&
      chooser.directory.id !== chooser.sourceParent.id &&
      chooser.directory.id !== chooser.entry.directory?.id,
    );
  }

  get byteProgressPercent(): number | null {
    const progress = this.progress;
    if (!progress?.completedBytes || !progress.totalBytes) return null;
    const completed = BigInt(progress.completedBytes);
    const total = BigInt(progress.totalBytes);
    if (total === 0n) return 100;
    return Number((completed * 10_000n) / total) / 100;
  }

  async openMoveChooser(
    entry: FileEntrySummary,
    sourceParent: DirectoryRef,
    locations: readonly LocationSummary[],
  ): Promise<void> {
    if (this.controller || this.moveChooser || !entry.capabilities.move) return;
    this.errorMessage = null;
    this.moveChooser = {
      entry,
      sourceParent,
      locations: [...locations],
      directory: sourceParent,
      parent: null,
      breadcrumbs: [],
      directories: [],
      loading: true,
      errorMessage: null,
    };
    await this.browseMoveDestination(sourceParent);
  }

  closeMoveChooser(): void {
    this.chooserController?.abort();
    this.chooserController = null;
    this.moveChooser = null;
  }

  async browseMoveDestination(directory: DirectoryRef): Promise<void> {
    const chooser = this.moveChooser;
    if (!chooser || !this.isCompatibleDestination(directory)) return;
    this.chooserController?.abort();
    const controller = new AbortController();
    this.chooserController = controller;
    this.moveChooser = {
      ...chooser,
      directory,
      parent: null,
      breadcrumbs: [],
      directories: [],
      loading: true,
      errorMessage: null,
    };
    try {
      await this.dataSource.listDirectory(directory, {
        signal: controller.signal,
        onStart: ({ directory, parent, breadcrumbs }) => {
          if (this.chooserController !== controller || !this.moveChooser)
            return;
          this.moveChooser = {
            ...this.moveChooser,
            directory,
            parent,
            breadcrumbs: [...breadcrumbs],
            directories: [],
          };
        },
        onBatch: ({ entries, replace }) => {
          if (this.chooserController !== controller || !this.moveChooser)
            return;
          const directories = entries.filter(
            (entry) => entry.directory !== null,
          );
          this.moveChooser = {
            ...this.moveChooser,
            directories: replace
              ? directories
              : [...this.moveChooser.directories, ...directories],
          };
        },
        onComplete: () => {},
      });
    } catch (error) {
      if (!isAbortError(error) && this.chooserController === controller) {
        this.moveChooser = this.moveChooser
          ? {
              ...this.moveChooser,
              errorMessage:
                error instanceof Error
                  ? error.message
                  : "Explora could not load that destination.",
            }
          : null;
      }
    } finally {
      if (this.chooserController === controller) {
        this.chooserController = null;
        if (this.moveChooser) {
          this.moveChooser = { ...this.moveChooser, loading: false };
        }
      }
    }
  }

  async confirmMove(
    onCompleted: (result: FileMoveResult) => void | Promise<void>,
  ): Promise<void> {
    const chooser = this.moveChooser;
    if (!chooser || !this.canConfirmMove) return;
    const { entry, directory } = chooser;
    this.closeMoveChooser();
    await this.runOperation(
      entry,
      "Moving",
      (options) => this.dataSource.moveEntry(entry, directory, options),
      onCompleted,
    );
  }

  async moveToTrash(
    entry: FileEntrySummary,
    onRemoved: (result: FileRemovalResult) => void | Promise<void>,
  ): Promise<void> {
    await this.runOperation(
      entry,
      "Moving to Trash",
      (options) => this.dataSource.trashEntry(entry, options),
      onRemoved,
    );
  }

  async deletePermanently(
    entry: FileEntrySummary,
    onRemoved: (result: FileRemovalResult) => void | Promise<void>,
  ): Promise<void> {
    await this.runOperation(
      entry,
      "Deleting permanently",
      (options) => this.dataSource.deleteEntryPermanently(entry, options),
      onRemoved,
    );
  }

  async answerPrompt(response: FileOperationPromptResponse): Promise<void> {
    const pending = this.pendingPrompt;
    if (!pending || pending.responding) return;
    this.pendingPrompt = { ...pending, responding: true };
    try {
      await pending.respond(response);
      if (this.pendingPrompt?.prompt.id === pending.prompt.id) {
        this.pendingPrompt = null;
      }
    } catch (error) {
      if (this.pendingPrompt?.prompt.id === pending.prompt.id) {
        this.pendingPrompt = { ...pending, responding: false };
      }
      this.errorMessage =
        error instanceof Error
          ? error.message
          : "Explora could not answer the filesystem decision.";
    }
  }

  cancelPrompt(): void {
    void this.answerPrompt("cancel");
  }

  cancelActive(): void {
    this.controller?.abort();
    this.controller = null;
    this.activeEntryId = null;
    this.activeEntryName = null;
    this.activeAction = null;
    this.progress = null;
    this.pendingPrompt = null;
  }

  clearError(): void {
    this.errorMessage = null;
  }

  isCompatibleDestination(directory: DirectoryRef): boolean {
    const chooser = this.moveChooser;
    return Boolean(
      chooser &&
      directory.capabilities.acceptMove &&
      directory.locationId === chooser.entry.reference.locationId &&
      directory.id !== chooser.entry.directory?.id,
    );
  }

  private async runOperation<T>(
    entry: FileEntrySummary,
    activeAction: string,
    start: (options: FileOperationOptions) => Promise<T>,
    onCompleted: (result: T) => void | Promise<void>,
  ): Promise<void> {
    if (this.controller) return;
    const controller = new AbortController();
    this.controller = controller;
    this.activeEntryId = entry.reference.id;
    this.activeEntryName = entry.name;
    this.activeAction = activeAction;
    this.progress = {
      completedItems: 0,
      totalItems: 1,
      completedBytes: null,
      totalBytes: null,
    };
    this.errorMessage = null;
    try {
      const result = await start({
        signal: controller.signal,
        onProgress: (progress) => {
          if (this.controller === controller) this.progress = progress;
        },
        onPrompt: (prompt, respond) => {
          if (this.controller !== controller) return;
          this.pendingPrompt = { prompt, responding: false, respond };
        },
      });
      if (this.controller !== controller) return;
      await onCompleted(result);
    } catch (error) {
      if (!isAbortError(error) && this.controller === controller) {
        this.errorMessage =
          error instanceof Error
            ? error.message
            : "Explora could not complete the filesystem action.";
      }
    } finally {
      if (this.controller === controller) {
        this.controller = null;
        this.activeEntryId = null;
        this.activeEntryName = null;
        this.activeAction = null;
        this.progress = null;
        this.pendingPrompt = null;
      }
    }
  }
}
