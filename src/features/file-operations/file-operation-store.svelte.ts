import type {
  FileEntrySummary,
  FileOperationConfirmation,
  FileOperationPromptResponse,
  FileRemovalResult,
} from "$lib/contracts/explorer";
import type { ExplorerDataSource } from "$lib/data/explorer-data-source";

interface PendingConfirmation {
  confirmation: FileOperationConfirmation;
  responding: boolean;
  respond: (response: FileOperationPromptResponse) => Promise<void>;
}

const isAbortError = (error: unknown) =>
  error instanceof Error && error.name === "AbortError";

export class FileOperationStore {
  activeEntryId = $state<string | null>(null);
  pendingConfirmation = $state<PendingConfirmation | null>(null);
  errorMessage = $state<string | null>(null);

  private controller: AbortController | null = null;

  constructor(private readonly dataSource: ExplorerDataSource) {}

  async moveToTrash(
    entry: FileEntrySummary,
    onRemoved: (result: FileRemovalResult) => void | Promise<void>,
  ): Promise<void> {
    await this.runRemoval(
      entry,
      (options) => this.dataSource.trashEntry(entry, options),
      onRemoved,
    );
  }

  async deletePermanently(
    entry: FileEntrySummary,
    onRemoved: (result: FileRemovalResult) => void | Promise<void>,
  ): Promise<void> {
    await this.runRemoval(
      entry,
      (options) => this.dataSource.deleteEntryPermanently(entry, options),
      onRemoved,
    );
  }

  async answerConfirmation(
    response: FileOperationPromptResponse,
  ): Promise<void> {
    const pending = this.pendingConfirmation;
    if (!pending || pending.responding) return;
    this.pendingConfirmation = { ...pending, responding: true };
    try {
      await pending.respond(response);
      if (
        this.pendingConfirmation?.confirmation.id === pending.confirmation.id
      ) {
        this.pendingConfirmation = null;
      }
    } catch (error) {
      if (
        this.pendingConfirmation?.confirmation.id === pending.confirmation.id
      ) {
        this.pendingConfirmation = { ...pending, responding: false };
      }
      this.errorMessage =
        error instanceof Error
          ? error.message
          : "Explora could not answer the filesystem confirmation.";
    }
  }

  cancelConfirmation(): void {
    void this.answerConfirmation("cancel");
  }

  cancelActive(): void {
    this.controller?.abort();
    this.controller = null;
    this.activeEntryId = null;
    this.pendingConfirmation = null;
  }

  clearError(): void {
    this.errorMessage = null;
  }

  private async runRemoval(
    entry: FileEntrySummary,
    start: (options: {
      signal: AbortSignal;
      onConfirmation: (
        confirmation: FileOperationConfirmation,
        respond: (response: FileOperationPromptResponse) => Promise<void>,
      ) => void;
    }) => Promise<FileRemovalResult>,
    onRemoved: (result: FileRemovalResult) => void | Promise<void>,
  ): Promise<void> {
    if (this.controller) return;
    const controller = new AbortController();
    this.controller = controller;
    this.activeEntryId = entry.reference.id;
    this.errorMessage = null;
    try {
      const result = await start({
        signal: controller.signal,
        onConfirmation: (confirmation, respond) => {
          if (this.controller !== controller) return;
          this.pendingConfirmation = {
            confirmation,
            responding: false,
            respond,
          };
        },
      });
      if (this.controller !== controller) return;
      await onRemoved(result);
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
        this.pendingConfirmation = null;
      }
    }
  }
}
