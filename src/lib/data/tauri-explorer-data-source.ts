import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  BreadcrumbSegment,
  ContentKind,
  DirectoryRef,
  EntryKind,
  FileEntrySummary,
  LocationKind,
  LocationStatus,
  LocationSummary,
  PreviewSummary,
} from "$lib/contracts/explorer";
import type {
  ExplorerDataSource,
  ListDirectoryOptions,
} from "$lib/data/explorer-data-source";
import { formatFileSize } from "$lib/file-metadata";

const contentKinds = new Set<ContentKind>([
  "folder",
  "image",
  "document",
  "code",
  "audio",
  "video",
  "archive",
  "other",
]);
const entryKinds = new Set<EntryKind>([
  "directory",
  "file",
  "symlink",
  "other",
]);
const locationKinds = new Set<LocationKind>(["local", "volume", "ssh"]);
const locationStatuses = new Set<LocationStatus>([
  "available",
  "connected",
  "offline",
]);

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const requireString = (
  record: Record<string, unknown>,
  key: string,
): string => {
  const value = record[key];
  if (typeof value !== "string") {
    throw new Error(`Invalid filesystem response: ${key} must be a string.`);
  }
  return value;
};

const parseDirectoryRef = (value: unknown): DirectoryRef => {
  if (!isRecord(value)) {
    throw new Error(
      "Invalid filesystem response: missing directory reference.",
    );
  }

  return {
    id: requireString(value, "id"),
    locationId: requireString(value, "locationId"),
    name: requireString(value, "name"),
    displayPath: requireString(value, "displayPath"),
  };
};

const parseLocation = (value: unknown): LocationSummary => {
  if (!isRecord(value)) {
    throw new Error("Invalid filesystem response: location must be an object.");
  }

  const kind = requireString(value, "kind");
  const status = requireString(value, "status");
  if (!locationKinds.has(kind as LocationKind)) {
    throw new Error(
      `Invalid filesystem response: unknown location kind ${kind}.`,
    );
  }
  if (!locationStatuses.has(status as LocationStatus)) {
    throw new Error(
      `Invalid filesystem response: unknown location status ${status}.`,
    );
  }

  return {
    id: requireString(value, "id"),
    name: requireString(value, "name"),
    kind: kind as LocationKind,
    status: status as LocationStatus,
    displayPath: requireString(value, "displayPath"),
    detail: requireString(value, "detail"),
    root: parseDirectoryRef(value.root),
  };
};

const parseEntry = (value: unknown): FileEntrySummary => {
  if (!isRecord(value) || !isRecord(value.reference)) {
    throw new Error("Invalid filesystem response: entry must be an object.");
  }

  const kind = requireString(value, "kind");
  const contentKind = requireString(value, "contentKind");
  if (!entryKinds.has(kind as EntryKind)) {
    throw new Error(`Invalid filesystem response: unknown entry kind ${kind}.`);
  }
  if (!contentKinds.has(contentKind as ContentKind)) {
    throw new Error(
      `Invalid filesystem response: unknown content kind ${contentKind}.`,
    );
  }
  if (
    value.size !== null &&
    (typeof value.size !== "string" || !/^\d+$/.test(value.size))
  ) {
    throw new Error("Invalid filesystem response: entry size is malformed.");
  }
  if (
    value.modifiedAt !== null &&
    (typeof value.modifiedAt !== "number" ||
      !Number.isSafeInteger(value.modifiedAt) ||
      value.modifiedAt < 0)
  ) {
    throw new Error(
      "Invalid filesystem response: modification time is malformed.",
    );
  }
  if (value.directory !== null && !isRecord(value.directory)) {
    throw new Error(
      "Invalid filesystem response: navigable directory is malformed.",
    );
  }
  if (
    value.detail !== undefined &&
    value.detail !== null &&
    typeof value.detail !== "string"
  ) {
    throw new Error("Invalid filesystem response: entry detail is malformed.");
  }

  return {
    reference: {
      id: requireString(value.reference, "id"),
      locationId: requireString(value.reference, "locationId"),
    },
    name: requireString(value, "name"),
    kind: kind as EntryKind,
    contentKind: contentKind as ContentKind,
    size: value.size,
    modifiedAt: value.modifiedAt,
    displayPath: requireString(value, "displayPath"),
    directory:
      value.directory === null ? null : parseDirectoryRef(value.directory),
    detail: typeof value.detail === "string" ? value.detail : undefined,
  };
};

const parseBreadcrumb = (value: unknown): BreadcrumbSegment => {
  if (!isRecord(value)) {
    throw new Error("Invalid filesystem response: breadcrumb is malformed.");
  }
  return {
    label: requireString(value, "label"),
    directory: parseDirectoryRef(value.directory),
  };
};

const abortError = () => {
  const error = new Error("The filesystem request was cancelled.");
  error.name = "AbortError";
  return error;
};

const commandError = (error: unknown): Error => {
  if (isRecord(error) && typeof error.message === "string") {
    const result = new Error(error.message);
    result.name =
      error.code === "cancelled" ? "AbortError" : "ExplorerFilesystemError";
    return result;
  }
  return error instanceof Error
    ? error
    : new Error("Explora could not complete the filesystem request.");
};

const requestId = () =>
  globalThis.crypto?.randomUUID?.() ??
  `listing-${Date.now()}-${Math.random().toString(16).slice(2)}`;

export class TauriExplorerDataSource implements ExplorerDataSource {
  async listLocations(
    signal: AbortSignal,
  ): Promise<readonly LocationSummary[]> {
    if (signal.aborted) throw abortError();
    const payload = await invoke<unknown>("list_local_locations");
    if (signal.aborted) throw abortError();
    if (!Array.isArray(payload)) {
      throw new Error("Invalid filesystem response: locations must be a list.");
    }
    return payload.map(parseLocation);
  }

  async listDirectory(
    directory: DirectoryRef,
    { signal, onStart, onBatch, onComplete }: ListDirectoryOptions,
  ): Promise<void> {
    if (signal.aborted) throw abortError();

    const id = requestId();
    let payloadError: Error | null = null;
    const channel = new Channel<unknown>();
    const cancel = () => {
      void invoke("cancel_local_listing", { requestId: id }).catch(() => {
        // Cancellation is best-effort; the stale-result guard remains authoritative.
      });
    };

    channel.onmessage = (payload) => {
      if (signal.aborted || payloadError) return;

      try {
        if (!isRecord(payload)) {
          throw new Error(
            "Invalid filesystem response: listing event is malformed.",
          );
        }

        if (payload.event === "started") {
          if (!Array.isArray(payload.breadcrumbs)) {
            throw new Error(
              "Invalid filesystem response: breadcrumbs must be a list.",
            );
          }
          onStart({
            directory: parseDirectoryRef(payload.directory),
            parent:
              payload.parent === null
                ? null
                : parseDirectoryRef(payload.parent),
            breadcrumbs: payload.breadcrumbs.map(parseBreadcrumb),
          });
        } else if (payload.event === "entries") {
          if (!Array.isArray(payload.entries)) {
            throw new Error(
              "Invalid filesystem response: entries must be a list.",
            );
          }
          if (typeof payload.replace !== "boolean") {
            throw new Error(
              "Invalid filesystem response: batch replacement flag is malformed.",
            );
          }
          onBatch({
            entries: payload.entries.map(parseEntry),
            replace: payload.replace,
          });
        } else if (payload.event === "complete") {
          if (
            typeof payload.skippedEntries !== "number" ||
            !Number.isInteger(payload.skippedEntries) ||
            payload.skippedEntries < 0
          ) {
            throw new Error(
              "Invalid filesystem response: skipped entry count is malformed.",
            );
          }
          onComplete({ skippedEntries: payload.skippedEntries });
        } else {
          throw new Error(
            "Invalid filesystem response: unknown listing event.",
          );
        }
      } catch (error) {
        payloadError =
          error instanceof Error
            ? error
            : new Error("Invalid filesystem response.");
        cancel();
      }
    };

    signal.addEventListener("abort", cancel, { once: true });
    try {
      await invoke("list_local_directory", {
        requestId: id,
        directoryId: directory.id,
        locationId: directory.locationId,
        onEvent: channel,
      });
      if (signal.aborted) throw abortError();
      if (payloadError) throw payloadError;
    } catch (error) {
      if (signal.aborted) throw abortError();
      if (payloadError) throw payloadError;
      throw commandError(error);
    } finally {
      signal.removeEventListener("abort", cancel);
    }
  }

  async getPreview(
    entry: FileEntrySummary,
    signal: AbortSignal,
  ): Promise<PreviewSummary> {
    if (signal.aborted) throw abortError();

    const kindLabel =
      entry.kind === "symlink"
        ? "Symbolic link"
        : entry.kind === "directory"
          ? "Folder"
          : "File";
    const details = [
      { label: "Path", value: entry.displayPath },
      { label: "Type", value: kindLabel },
      { label: "Size", value: formatFileSize(entry.size) },
    ];
    if (entry.modifiedAt !== null) {
      details.splice(2, 0, {
        label: "Modified",
        value: new Date(entry.modifiedAt).toLocaleString(),
      });
    }

    return {
      entryId: entry.reference.id,
      kind: entry.contentKind,
      title: entry.name,
      subtitle: entry.detail ?? kindLabel,
      details,
    };
  }
}
