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
  PreviewSummary,
  SshConnectionEvent,
  SshPromptResponse,
  SshTargetSource,
  SshTargetStatus,
  SshTargetSummary,
  VolumeSnapshot,
} from "$lib/contracts/explorer";
import type {
  ConnectSshOptions,
  ExplorerDataSource,
  ListDirectoryOptions,
  WatchVolumesOptions,
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

const parseBreadcrumb = (value: unknown): BreadcrumbSegment => {
  if (!isRecord(value)) {
    throw new Error("Invalid filesystem response: breadcrumb is malformed.");
  }
  return {
    label: requireString(value, "label"),
    directory: parseDirectoryRef(value.directory),
  };
};

const parseVolumeSnapshot = (value: unknown): VolumeSnapshot => {
  if (!isRecord(value)) {
    throw new Error("Invalid volume response: snapshot is malformed.");
  }
  if (
    typeof value.revision !== "number" ||
    !Number.isSafeInteger(value.revision) ||
    value.revision < 0
  ) {
    throw new Error("Invalid volume response: revision is malformed.");
  }
  if (!Array.isArray(value.volumes)) {
    throw new Error("Invalid volume response: volumes must be a list.");
  }
  if (value.warning !== null && typeof value.warning !== "string") {
    throw new Error("Invalid volume response: warning is malformed.");
  }
  const volumes = value.volumes.map(parseLocation);
  if (
    volumes.some(
      ({ id, kind, role, status }) =>
        !id.startsWith("volume:") ||
        kind !== "volume" ||
        role !== "volume" ||
        status !== "available",
    ) ||
    new Set(volumes.map(({ id }) => id)).size !== volumes.length
  ) {
    throw new Error("Invalid volume response: volume entries are malformed.");
  }
  return {
    revision: value.revision,
    volumes,
    warning: value.warning,
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
    const payload = await invoke<unknown>("list_locations");
    if (signal.aborted) throw abortError();
    if (!Array.isArray(payload)) {
      throw new Error("Invalid filesystem response: locations must be a list.");
    }
    return payload.map(parseLocation);
  }

  async watchVolumes({
    signal,
    onSnapshot,
  }: WatchVolumesOptions): Promise<void> {
    if (signal.aborted) throw abortError();
    const id = requestId();
    const channel = new Channel<unknown>();
    let rejectWatch: (error: Error) => void = () => {};
    const watching = new Promise<void>((resolve, reject) => {
      rejectWatch = reject;
      signal.addEventListener("abort", () => resolve(), { once: true });
    });
    const cancel = () => {
      void invoke("cancel_volume_watch", { requestId: id }).catch(() => {
        // Cancellation is best-effort; aborted snapshots are ignored below.
      });
    };
    channel.onmessage = (payload) => {
      if (signal.aborted) return;
      try {
        onSnapshot(parseVolumeSnapshot(payload));
      } catch (error) {
        cancel();
        rejectWatch(
          error instanceof Error
            ? error
            : new Error("Invalid volume response."),
        );
      }
    };
    signal.addEventListener("abort", cancel, { once: true });
    try {
      await invoke("watch_volumes", { requestId: id, onEvent: channel });
      await watching;
    } catch (error) {
      if (!signal.aborted) throw commandError(error);
    } finally {
      signal.removeEventListener("abort", cancel);
    }
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
      return parseSshTarget(payload);
    } catch (error) {
      if (signal.aborted) throw abortError();
      throw commandError(error);
    }
  }

  async deleteSshTarget(targetId: string, signal: AbortSignal): Promise<void> {
    await this.simpleSshCommand("delete_ssh_target", targetId, signal);
  }

  async disconnectSshTarget(
    targetId: string,
    signal: AbortSignal,
  ): Promise<void> {
    await this.simpleSshCommand("disconnect_ssh_target", targetId, signal);
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
        onEvent(event, async (response: SshPromptResponse) => {
          if (signal.aborted) throw abortError();
          try {
            await invoke("respond_ssh_prompt", {
              requestId: id,
              promptId: event.event === "state" ? "" : event.promptId,
              response,
            });
          } catch (error) {
            throw commandError(error);
          }
        });
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
      return parseLocation(payload);
    } catch (error) {
      if (signal.aborted) throw abortError();
      if (payloadError) throw payloadError;
      throw commandError(error);
    } finally {
      signal.removeEventListener("abort", cancel);
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
