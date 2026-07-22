import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DirectoryRef, FileEntrySummary } from "$lib/contracts/explorer";

import { TauriExplorerDataSource } from "./tauri-explorer-data-source";

const root: DirectoryRef = {
  id: "root-token",
  locationId: "home",
  name: "Home",
  displayPath: "/Users/test",
};

const locationPayload = {
  id: "home",
  name: "Home",
  backend: "local",
  kind: "local",
  role: "home",
  status: "available",
  displayPath: "/Users/test",
  detail: "Local",
  root,
  syncedFolder: null,
};

const entryPayload = {
  reference: { id: "entry-token", locationId: "home" },
  name: "notes.md",
  kind: "file",
  contentKind: "document",
  size: "5",
  modifiedAt: 1_721_324_000_000,
  displayPath: "/Users/test/notes.md",
  directory: null,
  availability: "local",
  detail: null,
};

const previewEntry: FileEntrySummary = {
  reference: entryPayload.reference,
  name: entryPayload.name,
  kind: "file",
  contentKind: "document",
  size: entryPayload.size,
  modifiedAt: entryPayload.modifiedAt,
  displayPath: entryPayload.displayPath,
  directory: null,
  availability: "local",
};

const sshTargetPayload = {
  id: "manual:target-1",
  locationId: "ssh:manual:target-1",
  name: "Staging",
  source: "manual",
  endpoint: "deploy@staging.example.com",
  status: "disconnected",
  editable: true,
  connectedLocationId: null,
  configuration: {
    name: "Staging",
    host: "staging.example.com",
    port: 22,
    username: "deploy",
    initialPath: "/srv/app",
    identityFile: null,
    identitiesOnly: false,
  },
};

const volumePayload = {
  ...locationPayload,
  id: "volume:test",
  name: "Test Volume",
  kind: "volume",
  role: "volume",
  displayPath: "/Volumes/Test Volume",
  detail: "750 GB available of 1 TB",
  root: {
    id: "volume-root-token",
    locationId: "volume:test",
    name: "Test Volume",
    displayPath: "/Volumes/Test Volume",
  },
};

const syncedFolderPayload = {
  ...locationPayload,
  id: "synced:icloud",
  name: "iCloud Drive",
  kind: "syncedFolder",
  role: "syncedFolder",
  displayPath: "/Users/test/Library/Mobile Documents/com~apple~CloudDocs",
  detail: "iCloud Drive · Synced folder",
  root: {
    id: "synced-root-token",
    locationId: "synced:icloud",
    name: "iCloud Drive",
    displayPath: "/Users/test/Library/Mobile Documents/com~apple~CloudDocs",
  },
  syncedFolder: {
    provider: "iCloud",
    status: "available",
    source: "system",
  },
};

const sendChannelMessages = (
  channel: unknown,
  messages: readonly unknown[],
  end = true,
) => {
  const toJson =
    typeof channel === "object" && channel !== null
      ? Reflect.get(channel, "toJSON")
      : undefined;
  const serialized =
    typeof toJson === "function"
      ? Reflect.apply(toJson, channel, [])
      : String(channel);
  const match = /^__CHANNEL__:(\d+)$/.exec(String(serialized));
  if (!match) throw new Error("Expected a Tauri channel identifier.");

  const internals = (
    window as unknown as {
      __TAURI_INTERNALS__: {
        runCallback: (id: number, payload: unknown) => void;
      };
    }
  ).__TAURI_INTERNALS__;
  const callbackId = Number(match[1]);
  messages.forEach((message, index) => {
    internals.runCallback(callbackId, { index, message });
  });
  if (end) {
    internals.runCallback(callbackId, { index: messages.length, end: true });
  }
};

afterEach(() => {
  clearMocks();
  vi.restoreAllMocks();
});

describe("TauriExplorerDataSource", () => {
  it("validates locations and streams typed directory events", async () => {
    mockIPC((command, payload) => {
      if (command === "list_locations") return [locationPayload];
      if (command === "list_directory") {
        if (
          !payload ||
          Array.isArray(payload) ||
          payload instanceof ArrayBuffer ||
          payload instanceof Uint8Array
        ) {
          throw new Error("Expected directory command arguments.");
        }
        sendChannelMessages(payload.onEvent, [
          {
            event: "started",
            directory: root,
            parent: null,
            breadcrumbs: [{ label: "Home", directory: root }],
          },
          { event: "entries", entries: [entryPayload], replace: true },
          { event: "complete", skippedEntries: 0 },
        ]);
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    const controller = new AbortController();
    await expect(source.listLocations(controller.signal)).resolves.toEqual([
      locationPayload,
    ]);
    const onStart = vi.fn();
    const onBatch = vi.fn();
    const onComplete = vi.fn();

    await source.listDirectory(root, {
      signal: controller.signal,
      onStart,
      onBatch,
      onComplete,
    });

    expect(onStart).toHaveBeenCalledWith({
      directory: root,
      parent: null,
      breadcrumbs: [{ label: "Home", directory: root }],
    });
    expect(onBatch).toHaveBeenCalledWith({
      entries: [{ ...entryPayload, detail: undefined }],
      replace: true,
    });
    expect(onComplete).toHaveBeenCalledWith({ skippedEntries: 0 });
  });

  it("rejects malformed IPC data before it reaches explorer state", async () => {
    mockIPC((command) => {
      if (command === "list_locations") {
        return [{ ...locationPayload, kind: "untrusted" }];
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    await expect(
      source.listLocations(new AbortController().signal),
    ).rejects.toThrow("unknown location kind");
  });

  it("accepts GIO only as a synced-folder transport", async () => {
    const gioFolder = {
      ...syncedFolderPayload,
      id: "synced:gio:opaque",
      backend: "gio",
      name: "Google Drive",
      displayPath: "Google Drive",
      root: {
        id: "gio-root-token",
        locationId: "synced:gio:opaque",
        name: "Google Drive",
        displayPath: "Google Drive",
      },
      syncedFolder: {
        provider: "googleDrive",
        status: "available",
        source: "system",
      },
    };
    mockIPC((command) => (command === "list_locations" ? [gioFolder] : null));

    await expect(
      new TauriExplorerDataSource().listLocations(new AbortController().signal),
    ).resolves.toEqual([gioFolder]);

    mockIPC((command) =>
      command === "list_locations"
        ? [{ ...locationPayload, backend: "gio" }]
        : null,
    );
    await expect(
      new TauriExplorerDataSource().listLocations(new AbortController().signal),
    ).rejects.toThrow("location backend does not match its kind");
  });

  it("rejects listing entries attributed to a different location", async () => {
    mockIPC((command, payload) => {
      if (command !== "list_directory") return null;
      if (!payload || typeof payload !== "object") {
        throw new Error("Expected directory command arguments.");
      }
      sendChannelMessages(Reflect.get(payload, "onEvent"), [
        {
          event: "entries",
          entries: [
            {
              ...entryPayload,
              reference: { ...entryPayload.reference, locationId: "forged" },
            },
          ],
          replace: true,
        },
      ]);
      return null;
    });

    await expect(
      new TauriExplorerDataSource().listDirectory(root, {
        signal: new AbortController().signal,
        onStart: () => {},
        onBatch: () => {},
        onComplete: () => {},
      }),
    ).rejects.toThrow("entry location identity does not match");
  });

  it("rejects unknown semantic location roles", async () => {
    mockIPC((command) => {
      if (command === "list_locations") {
        return [{ ...locationPayload, role: "untrusted" }];
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    await expect(
      source.listLocations(new AbortController().signal),
    ).rejects.toThrow("unknown location role");
  });

  it("rejects location roles and roots that do not match their identity", async () => {
    mockIPC((command) => {
      if (command === "list_locations") {
        return [{ ...locationPayload, role: "volume" }];
      }
      return null;
    });
    const source = new TauriExplorerDataSource();
    await expect(
      source.listLocations(new AbortController().signal),
    ).rejects.toThrow("location role does not match");

    clearMocks();
    mockIPC((command) => {
      if (command === "list_locations") {
        return [
          {
            ...locationPayload,
            root: { ...locationPayload.root, locationId: "forged" },
          },
        ];
      }
      return null;
    });
    await expect(
      source.listLocations(new AbortController().signal),
    ).rejects.toThrow("root identity does not match");
  });

  it("streams validated volume snapshots and cancels the Rust watch", async () => {
    const commands: string[] = [];
    mockIPC((command, payload) => {
      commands.push(command);
      if (command === "watch_volumes") {
        if (
          !payload ||
          Array.isArray(payload) ||
          payload instanceof ArrayBuffer ||
          payload instanceof Uint8Array
        ) {
          throw new Error("Expected volume watch arguments.");
        }
        sendChannelMessages(payload.onEvent, [
          { revision: 2, volumes: [volumePayload], warning: null },
        ]);
      }
      return null;
    });
    const source = new TauriExplorerDataSource();
    const controller = new AbortController();
    const onSnapshot = vi.fn(() => controller.abort());

    await source.watchVolumes({ signal: controller.signal, onSnapshot });

    expect(onSnapshot).toHaveBeenCalledWith({
      revision: 2,
      volumes: [volumePayload],
      warning: null,
    });
    expect(commands).toContain("cancel_volume_watch");
  });

  it("streams validated synced-folder snapshots and cancels the Rust watch", async () => {
    const commands: string[] = [];
    mockIPC((command, payload) => {
      commands.push(command);
      if (command === "watch_synced_folders") {
        if (
          !payload ||
          Array.isArray(payload) ||
          payload instanceof ArrayBuffer ||
          payload instanceof Uint8Array
        ) {
          throw new Error("Expected synced-folder watch arguments.");
        }
        sendChannelMessages(payload.onEvent, [
          {
            revision: 3,
            folders: [syncedFolderPayload],
            warning: null,
            canAddFolder: true,
          },
        ]);
      }
      return null;
    });
    const source = new TauriExplorerDataSource();
    const controller = new AbortController();
    const onSnapshot = vi.fn(() => controller.abort());

    await source.watchSyncedFolders({ signal: controller.signal, onSnapshot });

    expect(onSnapshot).toHaveBeenCalledWith({
      revision: 3,
      folders: [syncedFolderPayload],
      warning: null,
      canAddFolder: true,
    });
    expect(commands).toContain("cancel_synced_folder_watch");
  });

  it("rejects mismatched synced-folder metadata", async () => {
    mockIPC((command, payload) => {
      if (command === "watch_synced_folders") {
        if (!payload || typeof payload !== "object") {
          throw new Error("Expected synced-folder watch arguments.");
        }
        sendChannelMessages(Reflect.get(payload, "onEvent"), [
          {
            revision: 1,
            folders: [{ ...syncedFolderPayload, syncedFolder: null }],
            warning: null,
            canAddFolder: false,
          },
        ]);
      }
      return null;
    });
    const controller = new AbortController();

    await expect(
      new TauriExplorerDataSource().watchSyncedFolders({
        signal: controller.signal,
        onSnapshot: () => {},
      }),
    ).rejects.toThrow("synced-folder metadata is missing");
  });

  it("adds and removes only opaque manual synced-folder IDs", async () => {
    const calls: Array<{ command: string; payload: unknown }> = [];
    mockIPC((command, payload) => {
      calls.push({ command, payload });
      if (command === "add_synced_folder") {
        return "synced:manual:5f4c234c-bc60-41f4-86e7-f43082f7d331";
      }
      return null;
    });
    const source = new TauriExplorerDataSource();
    const signal = new AbortController().signal;

    const id = await source.addSyncedFolder(signal);
    expect(id).toBe("synced:manual:5f4c234c-bc60-41f4-86e7-f43082f7d331");
    await source.removeSyncedFolder(id!, signal);
    expect(calls).toContainEqual({
      command: "remove_synced_folder",
      payload: { folderId: id },
    });
  });

  it("rejects malformed manual synced-folder command results", async () => {
    mockIPC((command) =>
      command === "add_synced_folder" ? "/Users/private/Cloud" : null,
    );

    await expect(
      new TauriExplorerDataSource().addSyncedFolder(
        new AbortController().signal,
      ),
    ).rejects.toThrow("folder ID is malformed");
    await expect(
      new TauriExplorerDataSource().removeSyncedFolder(
        "/home/person/private-cloud",
        new AbortController().signal,
      ),
    ).rejects.toThrow("folder ID is malformed");
  });

  it("forwards AbortSignal cancellation to the active Rust listing", async () => {
    let finishListing = () => {};
    const commands: string[] = [];
    mockIPC((command) => {
      commands.push(command);
      if (command === "list_directory") {
        return new Promise<void>((resolve) => {
          finishListing = resolve;
        });
      }
      if (command === "cancel_listing") finishListing();
      return null;
    });

    const source = new TauriExplorerDataSource();
    const controller = new AbortController();
    const listing = source.listDirectory(root, {
      signal: controller.signal,
      onStart: () => {},
      onBatch: () => {},
      onComplete: () => {},
    });

    controller.abort();

    await expect(listing).rejects.toMatchObject({ name: "AbortError" });
    expect(commands).toContain("cancel_listing");
  });

  it("validates and returns a bounded text preview", async () => {
    mockIPC((command) => {
      if (command === "prepare_preview") {
        return {
          entryId: previewEntry.reference.id,
          size: "5",
          modifiedAt: previewEntry.modifiedAt,
          content: {
            type: "text",
            text: "hello",
            truncated: true,
            encoding: "UTF-8",
          },
        };
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    const prepared = await source.getPreview(previewEntry, {
      signal: new AbortController().signal,
      imageMode: "direct",
    });

    expect(prepared.preview.content).toEqual({
      type: "text",
      text: "hello",
      truncated: true,
      encoding: "UTF-8",
    });
    expect(prepared.preview.accessibilityDescription).toBe(
      previewEntry.displayPath,
    );
    expect(prepared.preview.details).not.toContainEqual(
      expect.objectContaining({ label: "Path" }),
    );
    expect(prepared.preview.details).toContainEqual({
      label: "Encoding",
      value: "UTF-8",
    });
    expect(() => prepared.dispose()).not.toThrow();
  });

  it("validates an explicit content-request capability on preview metadata", async () => {
    mockIPC((command) =>
      command === "prepare_preview"
        ? {
            entryId: previewEntry.reference.id,
            size: previewEntry.size,
            modifiedAt: previewEntry.modifiedAt,
            content: {
              type: "metadata",
              reason: "downloadRequired",
              message: "Download this file explicitly.",
              requestContent: {
                intent: "downloadToPreview",
                providerWorkCancellable: false,
              },
            },
          }
        : null,
    );

    const prepared = await new TauriExplorerDataSource().getPreview(
      { ...previewEntry, availability: "onlineOnly" },
      {
        signal: new AbortController().signal,
        imageMode: "direct",
      },
    );

    expect(prepared.preview.content).toMatchObject({
      type: "metadata",
      reason: "downloadRequired",
      requestContent: {
        intent: "downloadToPreview",
        providerWorkCancellable: false,
      },
    });
  });

  it("streams validated content-request state and completion", async () => {
    const events: unknown[] = [];
    mockIPC((command, payload) => {
      if (command === "request_content") {
        if (!payload || typeof payload !== "object") {
          throw new Error("Expected content request arguments.");
        }
        sendChannelMessages(Reflect.get(payload, "onEvent"), [
          { event: "started", providerWorkCancellable: false },
          { event: "progress", availability: "onlineOnly" },
          { event: "progress", availability: "downloading" },
          { event: "complete", availability: "local" },
        ]);
      }
      return null;
    });

    await new TauriExplorerDataSource().requestContent(
      {
        ...previewEntry,
        reference: {
          ...previewEntry.reference,
          locationId: "synced:icloud",
        },
        availability: "onlineOnly",
      },
      {
        signal: new AbortController().signal,
        onEvent: (event) => events.push(event),
      },
    );

    expect(events).toEqual([
      { event: "started", providerWorkCancellable: false },
      { event: "progress", availability: "onlineOnly" },
      { event: "progress", availability: "downloading" },
      { event: "complete", availability: "local" },
    ]);
  });

  it("stops waiting for an active content request through Rust", async () => {
    let finishRequest = () => {};
    let eventChannel: unknown;
    const commands: string[] = [];
    mockIPC((command, payload) => {
      commands.push(command);
      if (command === "request_content") {
        if (!payload || typeof payload !== "object") {
          throw new Error("Expected content request arguments.");
        }
        eventChannel = Reflect.get(payload, "onEvent");
        return new Promise<void>((resolve) => {
          finishRequest = resolve;
        });
      }
      if (command === "cancel_content_request") finishRequest();
      return null;
    });
    const source = new TauriExplorerDataSource();
    const controller = new AbortController();
    const request = source.requestContent(previewEntry, {
      signal: controller.signal,
      onEvent: () => {},
    });

    controller.abort();
    expect(commands).not.toContain("cancel_content_request");
    sendChannelMessages(
      eventChannel,
      [{ event: "started", providerWorkCancellable: false }],
      false,
    );

    await expect(request).rejects.toMatchObject({ name: "AbortError" });
    expect(commands).toContain("cancel_content_request");
  });

  it("loads raw image bytes and revokes the Blob URL on disposal", async () => {
    const createObjectUrl = vi
      .spyOn(URL, "createObjectURL")
      .mockReturnValue("blob:preview-1");
    const revokeObjectUrl = vi.spyOn(URL, "revokeObjectURL");
    const commands: string[] = [];
    let requestedImageMode: unknown;
    mockIPC((command, payload) => {
      commands.push(command);
      if (command === "prepare_preview") {
        requestedImageMode =
          payload && !Array.isArray(payload)
            ? Reflect.get(payload, "imageMode")
            : undefined;
        return {
          entryId: previewEntry.reference.id,
          size: "68",
          modifiedAt: previewEntry.modifiedAt,
          content: {
            type: "image",
            resourceId: "resource-1",
            mediaType: "image/png",
            imageMode: "direct",
            width: 640,
            height: 480,
            originalWidth: 4_032,
            originalHeight: 3_024,
          },
        };
      }
      if (command === "read_preview_resource") {
        return new Uint8Array([0x89, 0x50, 0x4e, 0x47]).buffer;
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    const prepared = await source.getPreview(
      { ...previewEntry, contentKind: "image", name: "photo.png" },
      {
        signal: new AbortController().signal,
        imageMode: "direct",
      },
    );

    expect(prepared.preview.content).toMatchObject({
      type: "image",
      url: "blob:preview-1",
      mediaType: "image/png",
      imageMode: "direct",
      width: 640,
      height: 480,
    });
    expect(commands).toContain("read_preview_resource");
    expect(requestedImageMode).toBe("direct");
    expect(createObjectUrl).toHaveBeenCalledOnce();
    prepared.dispose();
    prepared.dispose();
    expect(revokeObjectUrl).toHaveBeenCalledOnce();
  });

  it("loads bounded PDF bytes without creating a Blob URL", async () => {
    const createObjectUrl = vi.spyOn(URL, "createObjectURL");
    const commands: string[] = [];
    mockIPC((command) => {
      commands.push(command);
      if (command === "prepare_preview") {
        return {
          entryId: previewEntry.reference.id,
          size: "18",
          modifiedAt: previewEntry.modifiedAt,
          content: {
            type: "pdf",
            resourceId: "pdf-resource-1",
            mediaType: "application/pdf",
          },
        };
      }
      if (command === "read_preview_resource") {
        return new Uint8Array([
          0x25, 0x50, 0x44, 0x46, 0x2d, 0x31, 0x2e, 0x37, 0x0a,
        ]).buffer;
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    const prepared = await source.getPreview(
      { ...previewEntry, name: "brief.pdf" },
      {
        signal: new AbortController().signal,
        imageMode: "direct",
      },
    );

    expect(prepared.preview.content).toMatchObject({
      type: "pdf",
      mediaType: "application/pdf",
    });
    if (prepared.preview.content.type !== "pdf") {
      throw new Error("Expected PDF preview content.");
    }
    expect(
      Array.from(new Uint8Array(prepared.preview.content.data).slice(0, 5)),
    ).toEqual([0x25, 0x50, 0x44, 0x46, 0x2d]);
    expect(commands).toContain("read_preview_resource");
    expect(createObjectUrl).not.toHaveBeenCalled();
    expect(() => prepared.dispose()).not.toThrow();
  });

  it("rejects malformed preview payloads before rendering", async () => {
    mockIPC((command) =>
      command === "prepare_preview"
        ? {
            entryId: previewEntry.reference.id,
            size: "5",
            modifiedAt: null,
            content: { type: "image", resourceId: "resource-1" },
          }
        : null,
    );

    const source = new TauriExplorerDataSource();
    await expect(
      source.getPreview(previewEntry, {
        signal: new AbortController().signal,
        imageMode: "direct",
      }),
    ).rejects.toThrow("mediaType must be a string");
  });

  it("rejects an image prepared under a different rendering policy", async () => {
    const commands: string[] = [];
    mockIPC((command) => {
      commands.push(command);
      return command === "prepare_preview"
        ? {
            entryId: previewEntry.reference.id,
            size: "68",
            modifiedAt: previewEntry.modifiedAt,
            content: {
              type: "image",
              resourceId: "resource-1",
              mediaType: "image/png",
              imageMode: "sanitized",
              width: 640,
              height: 480,
              originalWidth: 640,
              originalHeight: 480,
            },
          }
        : null;
    });

    const source = new TauriExplorerDataSource();
    await expect(
      source.getPreview(previewEntry, {
        signal: new AbortController().signal,
        imageMode: "direct",
      }),
    ).rejects.toThrow("image mode does not match the request");
    expect(commands).toContain("discard_preview_resource");
  });

  it("forwards cancellation to an active Rust preview request", async () => {
    let finishPreview = () => {};
    const commands: string[] = [];
    mockIPC((command) => {
      commands.push(command);
      if (command === "prepare_preview") {
        return new Promise<void>((resolve) => {
          finishPreview = resolve;
        });
      }
      if (command === "cancel_preview") finishPreview();
      return null;
    });

    const source = new TauriExplorerDataSource();
    const controller = new AbortController();
    const preview = source.getPreview(previewEntry, {
      signal: controller.signal,
      imageMode: "direct",
    });
    controller.abort();

    await expect(preview).rejects.toMatchObject({ name: "AbortError" });
    expect(commands).toContain("cancel_preview");
  });

  it("validates saved SSH targets and their editable metadata", async () => {
    mockIPC((command) =>
      command === "list_ssh_targets" ? [sshTargetPayload] : null,
    );

    const source = new TauriExplorerDataSource();
    await expect(
      source.listSshTargets(new AbortController().signal),
    ).resolves.toEqual([sshTargetPayload]);
  });

  it("forwards host trust through a single-use SSH prompt response", async () => {
    const commands: string[] = [];
    let promptResponse: unknown;
    mockIPC((command, payload) => {
      commands.push(command);
      if (command === "connect_ssh_target") {
        if (
          !payload ||
          Array.isArray(payload) ||
          payload instanceof ArrayBuffer ||
          payload instanceof Uint8Array
        ) {
          throw new Error("Expected SSH command arguments.");
        }
        sendChannelMessages(payload.onEvent, [
          {
            event: "hostKeyPrompt",
            promptId: "prompt-1",
            host: "staging.example.com",
            port: 22,
            algorithm: "ssh-ed25519",
            fingerprint: "SHA256:test",
          },
        ]);
        return {
          ...locationPayload,
          backend: "ssh",
          kind: "ssh",
          role: "ssh",
        };
      }
      if (command === "respond_ssh_prompt") {
        promptResponse = payload;
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    const location = await source.connectSshTarget("manual:target-1", {
      signal: new AbortController().signal,
      onEvent: (event, respond) => {
        if (event.event === "hostKeyPrompt") {
          void respond({ response: "accept" });
        }
      },
    });
    await Promise.resolve();

    expect(location.kind).toBe("ssh");
    expect(commands).toContain("respond_ssh_prompt");
    expect(promptResponse).toMatchObject({
      promptId: "prompt-1",
      response: { response: "accept" },
    });
  });

  it("keeps the SSH event channel alive for disconnects after connect resolves", async () => {
    let eventChannel: unknown;
    mockIPC((command, payload) => {
      if (command === "connect_ssh_target") {
        if (
          !payload ||
          Array.isArray(payload) ||
          payload instanceof ArrayBuffer ||
          payload instanceof Uint8Array
        ) {
          throw new Error("Expected SSH command arguments.");
        }
        eventChannel = payload.onEvent;
        return {
          ...locationPayload,
          backend: "ssh",
          kind: "ssh",
          role: "ssh",
        };
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    const onEvent = vi.fn();
    await source.connectSshTarget("manual:target-1", {
      signal: new AbortController().signal,
      onEvent,
    });
    sendChannelMessages(
      eventChannel,
      [
        {
          event: "disconnected",
          targetId: "manual:target-1",
          message: "The SSH connection was lost.",
        },
      ],
      false,
    );

    expect(onEvent).toHaveBeenCalledWith(
      {
        event: "disconnected",
        targetId: "manual:target-1",
        message: "The SSH connection was lost.",
      },
      expect.any(Function),
    );
  });
});
