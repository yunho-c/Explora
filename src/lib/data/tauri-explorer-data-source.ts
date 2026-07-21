import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  BreadcrumbSegment,
  ContentKind,
  DirectoryRef,
  EntryKind,
  FileEntrySummary,
  LocationKind,
  LocationRole,
  LocationStatus,
  LocationSummary,
  ManualSshTargetInput,
  PreviewContent,
  PreviewSummary,
  PreviewUnavailableReason,
  SshConnectionEvent,
  SshPromptResponse,
  SshTargetSource,
  SshTargetStatus,
  SshTargetSummary,
} from "$lib/contracts/explorer";
import type {
  ConnectSshOptions,
  ExplorerDataSource,
  ListDirectoryOptions,
  PreparedPreview,
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
const locationRoles = new Set<LocationRole>([
  "home",
  "desktop",
  "documents",
  "downloads",
  "pictures",
  "music",
  "videos",
  "volume",
  "ssh",
]);
const locationStatuses = new Set<LocationStatus>([
  "available",
  "connected",
  "offline",
]);
const sshTargetSources = new Set<SshTargetSource>(["manual", "openSshConfig"]);
const sshTargetStatuses = new Set<SshTargetStatus>([
  "disconnected",
  "connecting",
  "connected",
  "error",
]);
const sshConnectionStates = new Set([
  "connecting",
  "authenticating",
  "openingSftp",
  "connected",
] as const);
const sshPromptKinds = new Set([
  "passphrase",
  "password",
  "keyboardInteractive",
] as const);
const previewUnavailableReasons = new Set<PreviewUnavailableReason>([
  "unsupported",
  "remote",
  "directory",
  "symlink",
  "tooLarge",
  "binary",
  "malformed",
  "timedOut",
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
  const role = requireString(value, "role");
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
  if (!locationRoles.has(role as LocationRole)) {
    throw new Error(
      `Invalid filesystem response: unknown location role ${role}.`,
    );
  }

  return {
    id: requireString(value, "id"),
    name: requireString(value, "name"),
    kind: kind as LocationKind,
    role: role as LocationRole,
    status: status as LocationStatus,
    displayPath: requireString(value, "displayPath"),
    detail: requireString(value, "detail"),
    root: parseDirectoryRef(value.root),
  };
};

const parseSshTarget = (value: unknown): SshTargetSummary => {
  if (!isRecord(value)) {
    throw new Error("Invalid SSH response: target must be an object.");
  }
  const source = requireString(value, "source");
  const status = requireString(value, "status");
  if (!sshTargetSources.has(source as SshTargetSource)) {
    throw new Error(`Invalid SSH response: unknown target source ${source}.`);
  }
  if (!sshTargetStatuses.has(status as SshTargetStatus)) {
    throw new Error(`Invalid SSH response: unknown target status ${status}.`);
  }
  if (typeof value.editable !== "boolean") {
    throw new Error("Invalid SSH response: editable must be a boolean.");
  }
  if (
    value.connectedLocationId !== null &&
    typeof value.connectedLocationId !== "string"
  ) {
    throw new Error("Invalid SSH response: connected location is malformed.");
  }
  const configuration = value.configuration;
  if (configuration !== null && !isRecord(configuration)) {
    throw new Error("Invalid SSH response: target configuration is malformed.");
  }
  const parsedConfiguration = configuration
    ? {
        name: requireString(configuration, "name"),
        host: requireString(configuration, "host"),
        port: configuration.port,
        username: requireString(configuration, "username"),
        initialPath:
          configuration.initialPath === null
            ? null
            : requireString(configuration, "initialPath"),
        identityFile:
          configuration.identityFile === null
            ? null
            : requireString(configuration, "identityFile"),
        identitiesOnly: configuration.identitiesOnly,
      }
    : null;
  if (
    parsedConfiguration &&
    (typeof parsedConfiguration.port !== "number" ||
      !Number.isInteger(parsedConfiguration.port) ||
      parsedConfiguration.port < 1 ||
      parsedConfiguration.port > 65_535 ||
      typeof parsedConfiguration.identitiesOnly !== "boolean")
  ) {
    throw new Error("Invalid SSH response: target configuration is malformed.");
  }
  return {
    id: requireString(value, "id"),
    locationId: requireString(value, "locationId"),
    name: requireString(value, "name"),
    source: source as SshTargetSource,
    endpoint: requireString(value, "endpoint"),
    status: status as SshTargetStatus,
    editable: value.editable,
    connectedLocationId: value.connectedLocationId,
    configuration: parsedConfiguration as ManualSshTargetInput | null,
  };
};

const parseSshConnectionEvent = (value: unknown): SshConnectionEvent => {
  if (!isRecord(value)) {
    throw new Error("Invalid SSH response: connection event is malformed.");
  }
  if (value.event === "state") {
    const state = requireString(value, "state");
    if (!sshConnectionStates.has(state as never)) {
      throw new Error(
        `Invalid SSH response: unknown connection state ${state}.`,
      );
    }
    return {
      event: "state",
      state: state as Extract<SshConnectionEvent, { event: "state" }>["state"],
    };
  }
  if (value.event === "hostKeyPrompt") {
    if (
      typeof value.port !== "number" ||
      !Number.isInteger(value.port) ||
      value.port < 1 ||
      value.port > 65_535
    ) {
      throw new Error("Invalid SSH response: host-key port is malformed.");
    }
    return {
      event: "hostKeyPrompt",
      promptId: requireString(value, "promptId"),
      host: requireString(value, "host"),
      port: value.port,
      algorithm: requireString(value, "algorithm"),
      fingerprint: requireString(value, "fingerprint"),
    };
  }
  if (value.event === "authenticationPrompt") {
    const kind = requireString(value, "kind");
    if (!sshPromptKinds.has(kind as never) || !Array.isArray(value.fields)) {
      throw new Error(
        "Invalid SSH response: authentication prompt is malformed.",
      );
    }
    return {
      event: "authenticationPrompt",
      promptId: requireString(value, "promptId"),
      kind: kind as "passphrase" | "password" | "keyboardInteractive",
      title: requireString(value, "title"),
      instructions: requireString(value, "instructions"),
      fields: value.fields.map((field) => {
        if (!isRecord(field) || typeof field.secret !== "boolean") {
          throw new Error("Invalid SSH response: prompt field is malformed.");
        }
        return { label: requireString(field, "label"), secret: field.secret };
      }),
    };
  }
  if (value.event === "disconnected") {
    return {
      event: "disconnected",
      targetId: requireString(value, "targetId"),
      message: requireString(value, "message"),
    };
  }
  throw new Error("Invalid SSH response: unknown connection event.");
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

interface PreparedPreviewPayload {
  entryId: string;
  size: string | null;
  modifiedAt: number | null;
  content:
    | {
        type: "metadata";
        reason: PreviewUnavailableReason;
        message: string;
      }
    | {
        type: "text";
        text: string;
        truncated: boolean;
        encoding: string;
      }
    | {
        type: "image";
        resourceId: string;
        mediaType: "image/png";
        width: number;
        height: number;
        originalWidth: number;
        originalHeight: number;
      };
}

const parseNullableSize = (value: unknown, context: string): string | null => {
  if (value === null) return null;
  if (typeof value !== "string" || !/^\d+$/.test(value)) {
    throw new Error(`Invalid preview response: ${context} is malformed.`);
  }
  return value;
};

const parseNullableTimestamp = (
  value: unknown,
  context: string,
): number | null => {
  if (value === null) return null;
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new Error(`Invalid preview response: ${context} is malformed.`);
  }
  return value;
};

const requirePreviewDimension = (
  record: Record<string, unknown>,
  key: string,
): number => {
  const value = record[key];
  if (
    typeof value !== "number" ||
    !Number.isSafeInteger(value) ||
    value < 1 ||
    value > 16_384
  ) {
    throw new Error(`Invalid preview response: ${key} is malformed.`);
  }
  return value;
};

const parsePreparedPreview = (
  value: unknown,
  expectedEntryId: string,
): PreparedPreviewPayload => {
  if (!isRecord(value) || !isRecord(value.content)) {
    throw new Error("Invalid preview response: preview must be an object.");
  }
  const entryId = requireString(value, "entryId");
  if (entryId !== expectedEntryId) {
    throw new Error(
      "Invalid preview response: entry reference does not match.",
    );
  }
  const size = parseNullableSize(value.size, "size");
  const modifiedAt = parseNullableTimestamp(value.modifiedAt, "modified time");
  const type = requireString(value.content, "type");

  if (type === "metadata") {
    const reason = requireString(value.content, "reason");
    if (!previewUnavailableReasons.has(reason as PreviewUnavailableReason)) {
      throw new Error(`Invalid preview response: unknown reason ${reason}.`);
    }
    return {
      entryId,
      size,
      modifiedAt,
      content: {
        type,
        reason: reason as PreviewUnavailableReason,
        message: requireString(value.content, "message"),
      },
    };
  }

  if (type === "text") {
    if (typeof value.content.truncated !== "boolean") {
      throw new Error("Invalid preview response: truncated flag is malformed.");
    }
    return {
      entryId,
      size,
      modifiedAt,
      content: {
        type,
        text: requireString(value.content, "text"),
        truncated: value.content.truncated,
        encoding: requireString(value.content, "encoding"),
      },
    };
  }

  if (type === "image") {
    const mediaType = requireString(value.content, "mediaType");
    if (mediaType !== "image/png") {
      throw new Error(`Invalid preview response: unsupported media type.`);
    }
    return {
      entryId,
      size,
      modifiedAt,
      content: {
        type,
        resourceId: requireString(value.content, "resourceId"),
        mediaType,
        width: requirePreviewDimension(value.content, "width"),
        height: requirePreviewDimension(value.content, "height"),
        originalWidth: requirePreviewDimension(value.content, "originalWidth"),
        originalHeight: requirePreviewDimension(
          value.content,
          "originalHeight",
        ),
      },
    };
  }

  throw new Error(`Invalid preview response: unknown content type ${type}.`);
};

const previewDetails = (
  entry: FileEntrySummary,
  payload: PreparedPreviewPayload,
): { details: PreviewSummary["details"]; subtitle: string } => {
  const size = payload.size ?? entry.size;
  const modifiedAt = payload.modifiedAt ?? entry.modifiedAt;
  const details: PreviewSummary["details"] = [
    { label: "Path", value: entry.displayPath },
    { label: "Size", value: formatFileSize(size) },
  ];
  if (modifiedAt !== null) {
    details.splice(1, 0, {
      label: "Modified",
      value: new Date(modifiedAt).toLocaleString(),
    });
  }

  if (payload.content.type === "text") {
    details.push({ label: "Encoding", value: payload.content.encoding });
    return {
      details,
      subtitle: entry.detail ?? `${payload.content.encoding} text`,
    };
  }
  if (payload.content.type === "image") {
    details.push({
      label: "Dimensions",
      value: `${payload.content.originalWidth} × ${payload.content.originalHeight}`,
    });
    return { details, subtitle: entry.detail ?? "Image" };
  }
  return {
    details,
    subtitle:
      entry.kind === "directory"
        ? (entry.detail ?? "Folder")
        : (entry.detail ?? "File"),
  };
};

const parsePreviewBytes = (value: unknown): Uint8Array => {
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (value instanceof Uint8Array) return value;
  throw new Error("Invalid preview response: image bytes are malformed.");
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
  private readonly sshChannels = new Map<string, Channel<unknown>>();

  async listLocations(
    signal: AbortSignal,
  ): Promise<readonly LocationSummary[]> {
    if (signal.aborted) throw abortError();
    const payload = await invoke<unknown>("list_locations");
    if (signal.aborted) throw abortError();
    if (!Array.isArray(payload)) {
      throw new Error("Invalid filesystem response: locations must be a list.");
    }
    return payload.map(parseLocation);
  }

  async listSshTargets(
    signal: AbortSignal,
  ): Promise<readonly SshTargetSummary[]> {
    if (signal.aborted) throw abortError();
    try {
      const payload = await invoke<unknown>("list_ssh_targets");
      if (signal.aborted) throw abortError();
      if (!Array.isArray(payload)) {
        throw new Error("Invalid SSH response: targets must be a list.");
      }
      return payload.map(parseSshTarget);
    } catch (error) {
      if (signal.aborted) throw abortError();
      throw commandError(error);
    }
  }

  async createSshTarget(
    input: ManualSshTargetInput,
    signal: AbortSignal,
  ): Promise<SshTargetSummary> {
    return this.saveSshTarget("create_ssh_target", null, input, signal);
  }

  async updateSshTarget(
    targetId: string,
    input: ManualSshTargetInput,
    signal: AbortSignal,
  ): Promise<SshTargetSummary> {
    return this.saveSshTarget("update_ssh_target", targetId, input, signal);
  }

  private async saveSshTarget(
    command: "create_ssh_target" | "update_ssh_target",
    targetId: string | null,
    input: ManualSshTargetInput,
    signal: AbortSignal,
  ): Promise<SshTargetSummary> {
    if (signal.aborted) throw abortError();
    try {
      const payload = await invoke<unknown>(command, {
        ...(targetId ? { targetId } : {}),
        input,
      });
      if (signal.aborted) throw abortError();
      const target = parseSshTarget(payload);
      if (targetId) this.sshChannels.delete(targetId);
      return target;
    } catch (error) {
      if (signal.aborted) throw abortError();
      throw commandError(error);
    }
  }

  async deleteSshTarget(targetId: string, signal: AbortSignal): Promise<void> {
    await this.simpleSshCommand("delete_ssh_target", targetId, signal);
    this.sshChannels.delete(targetId);
  }

  async disconnectSshTarget(
    targetId: string,
    signal: AbortSignal,
  ): Promise<void> {
    await this.simpleSshCommand("disconnect_ssh_target", targetId, signal);
    this.sshChannels.delete(targetId);
  }

  private async simpleSshCommand(
    command: "delete_ssh_target" | "disconnect_ssh_target",
    targetId: string,
    signal: AbortSignal,
  ): Promise<void> {
    if (signal.aborted) throw abortError();
    try {
      await invoke(command, { targetId });
      if (signal.aborted) throw abortError();
    } catch (error) {
      if (signal.aborted) throw abortError();
      throw commandError(error);
    }
  }

  async connectSshTarget(
    targetId: string,
    { signal, onEvent }: ConnectSshOptions,
  ): Promise<LocationSummary> {
    if (signal.aborted) throw abortError();
    const id = requestId();
    let payloadError: Error | null = null;
    let connected = false;
    const channel = new Channel<unknown>();
    const cancel = () => {
      void invoke("cancel_ssh_connection", { requestId: id }).catch(() => {
        // Connection cancellation is best-effort; stale state is still ignored.
      });
    };
    channel.onmessage = (payload) => {
      if (signal.aborted || payloadError) return;
      try {
        const event = parseSshConnectionEvent(payload);
        if (event.event === "disconnected") {
          if (event.targetId !== targetId) {
            throw new Error(
              "Invalid SSH response: disconnect target does not match the connection.",
            );
          }
          this.sshChannels.delete(event.targetId);
        }
        onEvent(event, async (response: SshPromptResponse) => {
          if (signal.aborted) throw abortError();
          try {
            await invoke("respond_ssh_prompt", {
              requestId: id,
              promptId: "promptId" in event ? event.promptId : "",
              response,
            });
          } catch (error) {
            throw commandError(error);
          }
        });
        if (event.event === "disconnected") {
          payloadError = new Error(event.message);
          cancel();
        }
      } catch (error) {
        payloadError =
          error instanceof Error ? error : new Error("Invalid SSH response.");
        cancel();
      }
    };

    signal.addEventListener("abort", cancel, { once: true });
    try {
      const payload = await invoke<unknown>("connect_ssh_target", {
        requestId: id,
        targetId,
        onEvent: channel,
      });
      if (signal.aborted) throw abortError();
      if (payloadError) throw payloadError;
      const location = parseLocation(payload);
      connected = true;
      if (!this.sshChannels.has(targetId)) {
        this.sshChannels.set(targetId, channel);
      }
      return location;
    } catch (error) {
      if (signal.aborted) throw abortError();
      if (payloadError) throw payloadError;
      throw commandError(error);
    } finally {
      signal.removeEventListener("abort", cancel);
      if (!connected) this.sshChannels.delete(targetId);
    }
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
      void invoke("cancel_listing", { requestId: id }).catch(() => {
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
      await invoke("list_directory", {
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
  ): Promise<PreparedPreview> {
    if (signal.aborted) throw abortError();
    const id = requestId();
    let pendingResourceId: string | null = null;
    let imageUrl: string | null = null;
    const cancel = () => {
      void invoke("cancel_preview", { requestId: id }).catch(() => {
        // Cancellation is best-effort; stale preview results are still ignored.
      });
      if (pendingResourceId) {
        void invoke("discard_preview_resource", {
          resourceId: pendingResourceId,
        }).catch(() => {
          // The resource may already have been consumed by the binary read.
        });
      }
    };

    signal.addEventListener("abort", cancel, { once: true });
    try {
      const rawPayload = await invoke<unknown>("prepare_preview", {
        requestId: id,
        entryId: entry.reference.id,
        locationId: entry.reference.locationId,
      });
      if (signal.aborted) throw abortError();
      const payload = parsePreparedPreview(rawPayload, entry.reference.id);
      const { details, subtitle } = previewDetails(entry, payload);
      let content: PreviewContent;

      if (payload.content.type === "image") {
        pendingResourceId = payload.content.resourceId;
        const rawBytes = await invoke<unknown>("read_preview_resource", {
          resourceId: pendingResourceId,
        });
        pendingResourceId = null;
        if (signal.aborted) throw abortError();
        const bytes = parsePreviewBytes(rawBytes);
        const ownedBytes = Uint8Array.from(bytes);
        const blob = new Blob([ownedBytes.buffer], {
          type: payload.content.mediaType,
        });
        imageUrl = URL.createObjectURL(blob);
        if (signal.aborted) throw abortError();
        content = {
          type: "image",
          url: imageUrl,
          mediaType: payload.content.mediaType,
          width: payload.content.width,
          height: payload.content.height,
          originalWidth: payload.content.originalWidth,
          originalHeight: payload.content.originalHeight,
        };
      } else {
        content = payload.content;
      }

      const preview: PreviewSummary = {
        entryId: entry.reference.id,
        kind: entry.contentKind,
        title: entry.name,
        subtitle,
        content,
        details,
      };
      let disposed = false;
      return {
        preview,
        dispose: () => {
          if (disposed) return;
          disposed = true;
          if (imageUrl) URL.revokeObjectURL(imageUrl);
        },
      };
    } catch (error) {
      if (pendingResourceId) {
        void invoke("discard_preview_resource", {
          resourceId: pendingResourceId,
        }).catch(() => {});
      }
      if (imageUrl) URL.revokeObjectURL(imageUrl);
      if (signal.aborted) throw abortError();
      throw commandError(error);
    } finally {
      signal.removeEventListener("abort", cancel);
    }
  }
}
