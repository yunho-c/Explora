import type {
  ContentAvailability,
  ContentKind,
  DirectoryRef,
  FileEntrySummary,
  LocationSummary,
  ManualSshTargetInput,
  PreviewContent,
  SshTargetSummary,
  SyncedFolderSnapshot,
} from "$lib/contracts/explorer";

import { createDemoPdf } from "./demo-pdf";
import type {
  ConnectSshOptions,
  ExplorerDataSource,
  ListDirectoryOptions,
  PreparePreviewOptions,
  PreparedPreview,
  RequestContentOptions,
  WatchSyncedFoldersOptions,
  WatchVolumesOptions,
} from "$lib/data/explorer-data-source";

const roots: Readonly<Record<string, DirectoryRef>> = {
  home: {
    id: "home",
    locationId: "home",
    name: "Home",
    displayPath: "Home",
  },
  desktop: {
    id: "desktop",
    locationId: "desktop",
    name: "Desktop",
    displayPath: "Home/Desktop",
  },
  documents: {
    id: "documents",
    locationId: "documents",
    name: "Documents",
    displayPath: "Home/Documents",
  },
  downloads: {
    id: "downloads",
    locationId: "downloads",
    name: "Downloads",
    displayPath: "Home/Downloads",
  },
  pictures: {
    id: "pictures",
    locationId: "pictures",
    name: "Pictures",
    displayPath: "Home/Pictures",
  },
  music: {
    id: "music",
    locationId: "music",
    name: "Music",
    displayPath: "Home/Music",
  },
  videos: {
    id: "videos",
    locationId: "videos",
    name: "Movies",
    displayPath: "Home/Movies",
  },
  workspace: {
    id: "workspace",
    locationId: "workspace",
    name: "Workspace",
    displayPath: "Workspace",
  },
  "synced:icloud": {
    id: "synced:icloud",
    locationId: "synced:icloud",
    name: "iCloud Drive",
    displayPath: "iCloud Drive",
  },
  "synced:onedrive": {
    id: "synced:onedrive",
    locationId: "synced:onedrive",
    name: "OneDrive",
    displayPath: "OneDrive",
  },
  "synced:google-drive": {
    id: "synced:google-drive",
    locationId: "synced:google-drive",
    name: "Google Drive",
    displayPath: "Google Drive",
  },
  "staging-box": {
    id: "staging-box",
    locationId: "staging-box",
    name: "staging-box",
    displayPath: "staging-box:~/projects",
  },
  "render-node": {
    id: "render-node",
    locationId: "render-node",
    name: "render-node",
    displayPath: "render-node:~",
  },
};

const locations: readonly LocationSummary[] = [
  {
    id: "home",
    name: "Home",
    backend: "local",
    kind: "local",
    role: "home",
    status: "available",
    displayPath: "Home",
    detail: "Local",
    root: roots.home,
    syncedFolder: null,
  },
  {
    id: "desktop",
    name: "Desktop",
    backend: "local",
    kind: "local",
    role: "desktop",
    status: "available",
    displayPath: "Home/Desktop",
    detail: "Local",
    root: roots.desktop,
    syncedFolder: null,
  },
  {
    id: "documents",
    name: "Documents",
    backend: "local",
    kind: "local",
    role: "documents",
    status: "available",
    displayPath: "Home/Documents",
    detail: "Local",
    root: roots.documents,
    syncedFolder: null,
  },
  {
    id: "downloads",
    name: "Downloads",
    backend: "local",
    kind: "local",
    role: "downloads",
    status: "available",
    displayPath: "Home/Downloads",
    detail: "Local",
    root: roots.downloads,
    syncedFolder: null,
  },
  {
    id: "pictures",
    name: "Pictures",
    backend: "local",
    kind: "local",
    role: "pictures",
    status: "available",
    displayPath: "Home/Pictures",
    detail: "Local",
    root: roots.pictures,
    syncedFolder: null,
  },
  {
    id: "music",
    name: "Music",
    backend: "local",
    kind: "local",
    role: "music",
    status: "available",
    displayPath: "Home/Music",
    detail: "Local",
    root: roots.music,
    syncedFolder: null,
  },
  {
    id: "videos",
    name: "Movies",
    backend: "local",
    kind: "local",
    role: "videos",
    status: "available",
    displayPath: "Home/Movies",
    detail: "Local",
    root: roots.videos,
    syncedFolder: null,
  },
  {
    id: "workspace",
    name: "Workspace",
    backend: "local",
    kind: "volume",
    role: "volume",
    status: "available",
    displayPath: "Workspace",
    detail: "1.2 TB available",
    root: roots.workspace,
    syncedFolder: null,
  },
  {
    id: "synced:icloud",
    name: "iCloud Drive",
    backend: "local",
    kind: "syncedFolder",
    role: "syncedFolder",
    status: "available",
    displayPath: "iCloud Drive",
    detail: "iCloud Drive · Synced folder",
    root: roots["synced:icloud"],
    syncedFolder: {
      provider: "iCloud",
      status: "available",
      source: "system",
    },
  },
  {
    id: "synced:onedrive",
    name: "OneDrive",
    backend: "local",
    kind: "syncedFolder",
    role: "syncedFolder",
    status: "available",
    displayPath: "OneDrive",
    detail: "OneDrive · Synced folder",
    root: roots["synced:onedrive"],
    syncedFolder: {
      provider: "oneDrive",
      status: "available",
      source: "system",
    },
  },
  {
    id: "synced:google-drive",
    name: "Google Drive",
    backend: "local",
    kind: "syncedFolder",
    role: "syncedFolder",
    status: "available",
    displayPath: "Google Drive",
    detail: "Google Drive · Synced folder",
    root: roots["synced:google-drive"],
    syncedFolder: {
      provider: "googleDrive",
      status: "available",
      source: "system",
    },
  },
  {
    id: "staging-box",
    name: "staging-box",
    backend: "ssh",
    kind: "ssh",
    role: "ssh",
    status: "connected",
    displayPath: "staging-box:~/projects",
    detail: "SSH · Connected",
    root: roots["staging-box"],
    syncedFolder: null,
  },
  {
    id: "render-node",
    name: "render-node",
    backend: "ssh",
    kind: "ssh",
    role: "ssh",
    status: "offline",
    displayPath: "render-node:~",
    detail: "SSH · Offline",
    root: roots["render-node"],
    syncedFolder: null,
  },
];

const makeEntry = (
  locationId: string,
  name: string,
  kind: FileEntrySummary["kind"],
  contentKind: ContentKind,
  size: number | null,
  modifiedAt: string,
  detail?: string,
  availability: ContentAvailability = "local",
): FileEntrySummary => ({
  reference: { id: `${locationId}:${name}`, locationId },
  name,
  kind,
  contentKind,
  size: size?.toString() ?? null,
  modifiedAt: Date.parse(modifiedAt),
  displayPath: `${roots[locationId].displayPath}/${name}`,
  directory:
    kind === "directory"
      ? {
          id: `${locationId}:${name}`,
          locationId,
          name,
          displayPath: `${roots[locationId].displayPath}/${name}`,
        }
      : null,
  availability,
  detail,
});

const entriesByLocation: Readonly<Record<string, readonly FileEntrySummary[]>> =
  {
    home: [
      makeEntry(
        "home",
        "Projects",
        "directory",
        "folder",
        null,
        "2026-07-18T18:42:00Z",
        "12 items",
      ),
      makeEntry(
        "home",
        "Photos",
        "directory",
        "folder",
        null,
        "2026-07-17T21:08:00Z",
        "328 items",
      ),
      makeEntry(
        "home",
        "Documents",
        "directory",
        "folder",
        null,
        "2026-07-16T16:14:00Z",
        "47 items",
      ),
      makeEntry(
        "home",
        "Downloads",
        "directory",
        "folder",
        null,
        "2026-07-18T17:54:00Z",
        "19 items",
      ),
      makeEntry(
        "home",
        "explora-notes.md",
        "file",
        "document",
        18_432,
        "2026-07-18T19:24:00Z",
      ),
      makeEntry(
        "home",
        "summer-light.jpg",
        "file",
        "image",
        4_284_811,
        "2026-07-15T03:36:00Z",
        "6240 × 4160",
      ),
      makeEntry(
        "home",
        "handoff.pdf",
        "file",
        "document",
        884_210,
        "2026-07-14T22:11:00Z",
        "8 pages",
      ),
      makeEntry(
        "home",
        "ambient-study.m4a",
        "file",
        "audio",
        8_916_320,
        "2026-07-12T08:44:00Z",
        "03:42",
      ),
      makeEntry(
        "home",
        "app-shell.svelte",
        "file",
        "code",
        12_104,
        "2026-07-18T18:06:00Z",
        "Svelte",
      ),
      makeEntry(
        "home",
        "walkthrough.mov",
        "file",
        "video",
        72_410_332,
        "2026-07-11T11:32:00Z",
        "00:48",
      ),
    ],
    desktop: [
      makeEntry(
        "desktop",
        "Screenshots",
        "directory",
        "folder",
        null,
        "2026-07-18T19:03:00Z",
        "24 items",
      ),
      makeEntry(
        "desktop",
        "release-checklist.md",
        "file",
        "document",
        6_904,
        "2026-07-18T18:18:00Z",
      ),
      makeEntry(
        "desktop",
        "layout-reference.png",
        "file",
        "image",
        1_244_019,
        "2026-07-18T16:25:00Z",
        "1440 × 900",
      ),
    ],
    documents: [
      makeEntry(
        "documents",
        "Design",
        "directory",
        "folder",
        null,
        "2026-07-16T10:12:00Z",
        "8 items",
      ),
      makeEntry(
        "documents",
        "Receipts",
        "directory",
        "folder",
        null,
        "2026-07-01T07:41:00Z",
        "31 items",
      ),
      makeEntry(
        "documents",
        "Explora brief.pdf",
        "file",
        "document",
        642_850,
        "2026-07-18T14:20:00Z",
        "6 pages",
      ),
      makeEntry(
        "documents",
        "ssh-hosts.txt",
        "file",
        "document",
        1_204,
        "2026-07-10T09:52:00Z",
      ),
    ],
    downloads: [],
    pictures: [],
    music: [],
    videos: [],
    workspace: [
      makeEntry(
        "workspace",
        "Archive",
        "directory",
        "folder",
        null,
        "2026-06-28T12:08:00Z",
        "96 items",
      ),
      makeEntry(
        "workspace",
        "Footage",
        "directory",
        "folder",
        null,
        "2026-07-17T23:22:00Z",
        "42 items",
      ),
      makeEntry(
        "workspace",
        "explora-backup.tar.zst",
        "file",
        "archive",
        412_884_992,
        "2026-07-18T02:00:00Z",
      ),
    ],
    "synced:icloud": [
      makeEntry(
        "synced:icloud",
        "Desktop",
        "directory",
        "folder",
        null,
        "2026-07-21T20:12:00Z",
        "Synced · 18 items",
      ),
      makeEntry(
        "synced:icloud",
        "Trip notes.md",
        "file",
        "document",
        12_430,
        "2026-07-21T18:44:00Z",
        "Available offline",
      ),
      makeEntry(
        "synced:icloud",
        "Reference library.pdf",
        "file",
        "document",
        8_420_112,
        "2026-07-20T09:30:00Z",
        "Online only",
        "onlineOnly",
      ),
    ],
    "synced:onedrive": [
      makeEntry(
        "synced:onedrive",
        "Shared",
        "directory",
        "folder",
        null,
        "2026-07-21T17:30:00Z",
        "Synced · 24 items",
      ),
      makeEntry(
        "synced:onedrive",
        "Quarterly plan.docx",
        "file",
        "document",
        244_032,
        "2026-07-21T16:52:00Z",
        "Online only",
        "onlineOnly",
      ),
    ],
    "synced:google-drive": [
      makeEntry(
        "synced:google-drive",
        "My Drive",
        "directory",
        "folder",
        null,
        "2026-07-21T22:10:00Z",
        "Synced · 31 items",
      ),
      makeEntry(
        "synced:google-drive",
        "Project handoff.pdf",
        "file",
        "document",
        1_284_992,
        "2026-07-21T21:15:00Z",
        "Available offline",
      ),
    ],
    "staging-box": [
      makeEntry(
        "staging-box",
        "explora",
        "directory",
        "folder",
        null,
        "2026-07-18T18:38:00Z",
        "SSH · 22 items",
      ),
      makeEntry(
        "staging-box",
        "deploy",
        "directory",
        "folder",
        null,
        "2026-07-18T17:12:00Z",
        "SSH · 7 items",
      ),
      makeEntry(
        "staging-box",
        "README.md",
        "file",
        "document",
        4_280,
        "2026-07-18T18:41:00Z",
        "SSH",
      ),
      makeEntry(
        "staging-box",
        "service.log",
        "file",
        "document",
        244_119,
        "2026-07-18T19:30:00Z",
        "SSH",
      ),
      makeEntry(
        "staging-box",
        "healthcheck.ts",
        "file",
        "code",
        3_822,
        "2026-07-18T16:52:00Z",
        "SSH · TypeScript",
      ),
    ],
    "render-node": [],
  };

const excerpts: Partial<Record<ContentKind, string>> = {
  document:
    "Explora keeps local and remote files in one calm, consistent workspace. This preview is representative demo content.",
  code: 'export const explorer = createExplorer({\n  local: true,\n  remotes: "first-class",\n});',
  image:
    "Image preview rendering will be supplied by the bounded Rust preview pipeline.",
  audio:
    "Audio · Preview playback will use the packaged platform media capabilities.",
  video:
    "Video · Preview playback will use the packaged platform media capabilities.",
  archive:
    "Archive contents are not expanded in the initial preview experience.",
};

const demoImageUrl =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

const abortError = () => {
  const error = new Error("The demo request was cancelled.");
  error.name = "AbortError";
  return error;
};

const wait = (duration: number, signal: AbortSignal) =>
  new Promise<void>((resolve, reject) => {
    if (signal.aborted) {
      reject(abortError());
      return;
    }

    const timeout = window.setTimeout(resolve, duration);
    signal.addEventListener(
      "abort",
      () => {
        window.clearTimeout(timeout);
        reject(abortError());
      },
      { once: true },
    );
  });

export class DemoExplorerDataSource implements ExplorerDataSource {
  private sshTargets: SshTargetSummary[] = [
    {
      id: "demo:staging-box",
      locationId: "staging-box",
      name: "staging-box",
      source: "manual",
      endpoint: "deploy@staging.example.com",
      status: "connected",
      editable: true,
      connectedLocationId: "staging-box",
      configuration: {
        name: "staging-box",
        host: "staging.example.com",
        port: 22,
        username: "deploy",
        initialPath: "~/projects",
        identityFile: null,
        identitiesOnly: false,
      },
    },
    {
      id: "demo:render-node",
      locationId: "render-node",
      name: "render-node",
      source: "openSshConfig",
      endpoint: "yunho@render.example.com",
      status: "disconnected",
      editable: false,
      connectedLocationId: null,
      configuration: null,
    },
  ];
  private dynamicLocations = new Map<string, LocationSummary>();
  private dynamicRoots = new Map<string, DirectoryRef>();
  private hydratedEntryIds = new Set<string>();

  async listLocations(
    signal: AbortSignal,
  ): Promise<readonly LocationSummary[]> {
    await wait(40, signal);
    return locations;
  }

  async watchVolumes({
    signal,
    onSnapshot,
  }: WatchVolumesOptions): Promise<void> {
    if (signal.aborted) throw abortError();
    onSnapshot({
      revision: 1,
      volumes: locations.filter(({ kind }) => kind === "volume"),
      warning: null,
    });
    await new Promise<void>((resolve) => {
      signal.addEventListener("abort", () => resolve(), { once: true });
    });
  }

  async watchSyncedFolders({
    signal,
    onSnapshot,
  }: WatchSyncedFoldersOptions): Promise<void> {
    if (signal.aborted) throw abortError();
    const snapshot: SyncedFolderSnapshot = {
      revision: 1,
      folders: locations.filter(({ kind }) => kind === "syncedFolder"),
      warning: null,
      canAddFolder: false,
    };
    onSnapshot(snapshot);
    await new Promise<void>((resolve) => {
      signal.addEventListener("abort", () => resolve(), { once: true });
    });
  }

  async addSyncedFolder(signal: AbortSignal): Promise<string | null> {
    await wait(0, signal);
    return null;
  }

  async removeSyncedFolder(
    _folderId: string,
    signal: AbortSignal,
  ): Promise<void> {
    await wait(0, signal);
    throw new Error("Demo synced folders cannot be removed.");
  }

  async listSshTargets(
    signal: AbortSignal,
  ): Promise<readonly SshTargetSummary[]> {
    await wait(30, signal);
    return this.sshTargets.map((target) => ({ ...target }));
  }

  async createSshTarget(
    input: ManualSshTargetInput,
    signal: AbortSignal,
  ): Promise<SshTargetSummary> {
    await wait(40, signal);
    const id = `demo:manual:${Date.now()}`;
    const target: SshTargetSummary = {
      id,
      locationId: `ssh:${id}`,
      name: input.name,
      source: "manual",
      endpoint: `${input.username}@${input.host}${input.port === 22 ? "" : `:${input.port}`}`,
      status: "disconnected",
      editable: true,
      connectedLocationId: null,
      configuration: { ...input },
    };
    this.sshTargets = [...this.sshTargets, target];
    return { ...target };
  }

  async updateSshTarget(
    targetId: string,
    input: ManualSshTargetInput,
    signal: AbortSignal,
  ): Promise<SshTargetSummary> {
    await wait(40, signal);
    const target = this.sshTargets.find(({ id }) => id === targetId);
    if (!target || !target.editable)
      throw new Error("Unknown demo SSH target.");
    const updated: SshTargetSummary = {
      ...target,
      name: input.name,
      endpoint: `${input.username}@${input.host}${input.port === 22 ? "" : `:${input.port}`}`,
      status: "disconnected",
      connectedLocationId: null,
      configuration: { ...input },
    };
    this.sshTargets = this.sshTargets.map((candidate) =>
      candidate.id === targetId ? updated : candidate,
    );
    return { ...updated };
  }

  async deleteSshTarget(targetId: string, signal: AbortSignal): Promise<void> {
    await wait(30, signal);
    this.sshTargets = this.sshTargets.filter(({ id }) => id !== targetId);
  }

  async connectSshTarget(
    targetId: string,
    { signal, onEvent }: ConnectSshOptions,
  ): Promise<LocationSummary> {
    const target = this.sshTargets.find(({ id }) => id === targetId);
    if (!target) throw new Error("Unknown demo SSH target.");
    onEvent({ event: "state", state: "connecting" }, async () => {});
    await wait(120, signal);
    onEvent({ event: "state", state: "authenticating" }, async () => {});
    await wait(100, signal);

    let location = locations.find(({ name }) => name === target.name);
    if (!location) {
      const locationId = `ssh:${target.id}`;
      const root: DirectoryRef = {
        id: locationId,
        locationId,
        name: target.name,
        displayPath: `${target.name}:~`,
      };
      location = {
        id: locationId,
        name: target.name,
        backend: "ssh",
        kind: "ssh",
        role: "ssh",
        status: "connected",
        displayPath: root.displayPath,
        detail: target.endpoint,
        root,
        syncedFolder: null,
      };
      this.dynamicRoots.set(locationId, root);
      this.dynamicLocations.set(locationId, location);
    } else {
      location = { ...location, status: "connected", detail: target.endpoint };
    }
    this.sshTargets = this.sshTargets.map((candidate) =>
      candidate.id === targetId
        ? {
            ...candidate,
            status: "connected",
            connectedLocationId: location!.id,
          }
        : candidate,
    );
    onEvent({ event: "state", state: "connected" }, async () => {});
    return location;
  }

  async disconnectSshTarget(
    targetId: string,
    signal: AbortSignal,
  ): Promise<void> {
    await wait(30, signal);
    this.sshTargets = this.sshTargets.map((target) =>
      target.id === targetId
        ? { ...target, status: "disconnected", connectedLocationId: null }
        : target,
    );
  }

  async listDirectory(
    directory: DirectoryRef,
    { signal, onStart, onBatch, onComplete }: ListDirectoryOptions,
  ): Promise<void> {
    const root =
      roots[directory.locationId] ??
      this.dynamicRoots.get(directory.locationId);
    const rootEntries = entriesByLocation[directory.id];
    const isKnownChild = Object.values(entriesByLocation)
      .flat()
      .some((entry) => entry.directory?.id === directory.id);
    const entries =
      rootEntries ??
      (this.dynamicRoots.has(directory.locationId) || isKnownChild
        ? []
        : undefined);

    if (!entries || !root) {
      throw new Error(`Unknown demo directory: ${directory.id}`);
    }

    await wait(90, signal);
    onStart({
      directory,
      parent: directory.id === root.id ? null : root,
      breadcrumbs:
        directory.id === root.id
          ? [{ label: root.name, directory: root }]
          : [
              { label: root.name, directory: root },
              { label: directory.name, directory },
            ],
    });
    const splitAt = Math.min(4, entries.length);
    if (splitAt > 0) {
      onBatch({ entries: entries.slice(0, splitAt), replace: true });
    }

    if (entries.length > splitAt) {
      await wait(110, signal);
      onBatch({ entries: entries.slice(splitAt), replace: false });
    }
    onComplete({ skippedEntries: 0 });
  }

  async getPreview(
    entry: FileEntrySummary,
    { signal, imageMode }: PreparePreviewOptions,
  ): Promise<PreparedPreview> {
    await wait(80, signal);

    const location = locations.find(
      ({ id }) => id === entry.reference.locationId,
    );
    let content: PreviewContent;
    if (
      entry.availability !== "local" &&
      !this.hydratedEntryIds.has(entry.reference.id)
    ) {
      content = {
        type: "metadata",
        reason: "downloadRequired",
        message: "Download this file before opening Quick Preview.",
        requestContent: {
          intent: "downloadToPreview",
          providerWorkCancellable: false,
        },
      };
    } else if (location?.backend === "ssh") {
      content = {
        type: "metadata",
        reason: "remote",
        message: "Remote content preview is not available yet.",
        requestContent: null,
      };
    } else if (entry.contentKind === "image") {
      content = {
        type: "image",
        url: demoImageUrl,
        mediaType: "image/png",
        imageMode,
        width: 960,
        height: 640,
        originalWidth: 4_032,
        originalHeight: 3_024,
      };
    } else if (entry.name.toLocaleLowerCase().endsWith(".pdf")) {
      content = {
        type: "pdf",
        data: createDemoPdf(),
        mediaType: "application/pdf",
      };
    } else if (
      entry.contentKind === "document" ||
      entry.contentKind === "code"
    ) {
      content = {
        type: "text",
        text: excerpts[entry.contentKind] ?? "",
        truncated: false,
        encoding: "UTF-8",
      };
    } else {
      content = {
        type: "metadata",
        reason: entry.kind === "directory" ? "directory" : "unsupported",
        message:
          excerpts[entry.contentKind] ??
          "Content preview is not available for this file type yet.",
        requestContent: null,
      };
    }
    return {
      preview: {
        entryId: entry.reference.id,
        kind: entry.contentKind,
        title: entry.name,
        accessibilityDescription: entry.displayPath,
        content,
        details: [
          {
            label: "Modified",
            value:
              entry.modifiedAt === null
                ? "Unknown"
                : new Date(entry.modifiedAt).toLocaleString(),
          },
          {
            label: "Size",
            value: entry.size === null ? "—" : `${entry.size} bytes`,
          },
        ],
      },
      dispose: () => {},
    };
  }

  async requestContent(
    entry: FileEntrySummary,
    { signal, onEvent }: RequestContentOptions,
  ): Promise<void> {
    if (entry.reference.locationId !== "synced:icloud") {
      throw new Error("This demo location cannot request cloud content.");
    }
    onEvent({ event: "started", providerWorkCancellable: false });
    onEvent({ event: "progress", availability: entry.availability });
    await wait(120, signal);
    onEvent({ event: "progress", availability: "downloading" });
    await wait(180, signal);
    this.hydratedEntryIds.add(entry.reference.id);
    onEvent({ event: "complete", availability: "local" });
  }
}
