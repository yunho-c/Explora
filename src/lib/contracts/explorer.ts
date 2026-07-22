export type LocationBackend = "local" | "ssh";
export type LocationKind = "local" | "volume" | "syncedFolder" | "ssh";
export type LocationRole =
  | "home"
  | "desktop"
  | "documents"
  | "downloads"
  | "pictures"
  | "music"
  | "videos"
  | "volume"
  | "syncedFolder"
  | "ssh";
export type LocationStatus = "available" | "connected" | "offline";
export type SyncedFolderProvider =
  "iCloud" | "oneDrive" | "googleDrive" | "other";
export type SyncedFolderStatus =
  "available" | "offline" | "paused" | "error" | "unknown";
export type ContentAvailability =
  | "local"
  | "onlineOnly"
  | "partial"
  | "downloading"
  | "syncing"
  | "error"
  | "unknown";
export type SyncedFolderSource = "system" | "manual";
export type SshTargetSource = "manual" | "openSshConfig";
export type SshTargetStatus =
  "disconnected" | "connecting" | "connected" | "error";
export type EntryKind = "directory" | "file" | "symlink" | "other";
export type ContentKind =
  | "folder"
  | "image"
  | "document"
  | "code"
  | "audio"
  | "video"
  | "archive"
  | "other";
export type ViewMode = "list" | "grid";
export type SortColumn = "name" | "modifiedAt" | "size";
export type SortDirection = "ascending" | "descending";
export type ImagePreviewMode = "direct" | "sanitized";
export type PreviewImageMediaType =
  "image/bmp" | "image/jpeg" | "image/png" | "image/webp";

export interface EntryRef {
  id: string;
  locationId: string;
}

export interface DirectoryRef extends EntryRef {
  name: string;
  displayPath: string;
}

export interface BreadcrumbSegment {
  label: string;
  directory: DirectoryRef;
}

export interface LocationSummary {
  id: string;
  name: string;
  backend: LocationBackend;
  kind: LocationKind;
  role: LocationRole;
  status: LocationStatus;
  displayPath: string;
  detail: string;
  root: DirectoryRef;
  syncedFolder: SyncedFolderMetadata | null;
}

export interface SyncedFolderMetadata {
  provider: SyncedFolderProvider;
  status: SyncedFolderStatus;
  source: SyncedFolderSource;
}

export interface VolumeSnapshot {
  revision: number;
  volumes: readonly LocationSummary[];
  warning: string | null;
}

export interface SyncedFolderSnapshot {
  revision: number;
  folders: readonly LocationSummary[];
  warning: string | null;
  canAddFolder: boolean;
}

export interface SshTargetSummary {
  id: string;
  locationId: string;
  name: string;
  source: SshTargetSource;
  endpoint: string;
  status: SshTargetStatus;
  editable: boolean;
  connectedLocationId: string | null;
  configuration: ManualSshTargetInput | null;
}

export interface ManualSshTargetInput {
  name: string;
  host: string;
  port: number;
  username: string;
  initialPath: string | null;
  identityFile: string | null;
  identitiesOnly: boolean;
}

export interface SshPromptField {
  label: string;
  secret: boolean;
}

export type SshConnectionEvent =
  | {
      event: "state";
      state: "connecting" | "authenticating" | "openingSftp" | "connected";
    }
  | {
      event: "hostKeyPrompt";
      promptId: string;
      host: string;
      port: number;
      algorithm: string;
      fingerprint: string;
    }
  | {
      event: "authenticationPrompt";
      promptId: string;
      kind: "passphrase" | "password" | "keyboardInteractive";
      title: string;
      instructions: string;
      fields: SshPromptField[];
    }
  | {
      event: "disconnected";
      targetId: string;
      message: string;
    };

export type SshPromptResponse =
  | { response: "accept" }
  | { response: "reject" }
  | { response: "answers"; answers: string[] };

export interface FileEntrySummary {
  reference: EntryRef;
  name: string;
  kind: EntryKind;
  contentKind: ContentKind;
  size: string | null;
  modifiedAt: number | null;
  displayPath: string;
  directory: DirectoryRef | null;
  availability: ContentAvailability;
  detail?: string;
}

export interface ExplorerTab {
  id: string;
  title: string;
  locationId: string;
  directory: DirectoryRef;
  history: DirectoryRef[];
  historyIndex: number;
}

export interface SortDescriptor {
  column: SortColumn;
  direction: SortDirection;
}

export interface PreviewDetail {
  label: string;
  value: string;
}

export type PreviewUnavailableReason =
  | "unsupported"
  | "downloadRequired"
  | "remote"
  | "directory"
  | "symlink"
  | "tooLarge"
  | "binary"
  | "malformed"
  | "timedOut";

export interface ContentRequestCapability {
  intent: "downloadToPreview";
  providerWorkCancellable: boolean;
}

export type ContentRequestEvent =
  | {
      event: "started";
      providerWorkCancellable: boolean;
    }
  | {
      event: "progress";
      availability: ContentAvailability;
    }
  | {
      event: "complete";
      availability: "local";
    };

export type PreviewContent =
  | {
      type: "metadata";
      reason: PreviewUnavailableReason;
      message: string;
      requestContent: ContentRequestCapability | null;
    }
  | {
      type: "text";
      text: string;
      truncated: boolean;
      encoding: string;
    }
  | {
      type: "image";
      url: string;
      mediaType: PreviewImageMediaType;
      imageMode: ImagePreviewMode;
      width: number;
      height: number;
      originalWidth: number;
      originalHeight: number;
    }
  | {
      type: "pdf";
      data: ArrayBuffer;
      mediaType: "application/pdf";
    };

export interface PreviewSummary {
  entryId: string;
  kind: ContentKind;
  title: string;
  accessibilityDescription: string;
  content: PreviewContent;
  details: PreviewDetail[];
}
