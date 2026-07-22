export type LocationKind = "local" | "volume" | "ssh";
export type LocationRole =
  | "home"
  | "desktop"
  | "documents"
  | "downloads"
  | "pictures"
  | "music"
  | "videos"
  | "volume"
  | "ssh";
export type LocationStatus = "available" | "connected" | "offline";
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
  capabilities: DirectoryCapabilities;
}

export interface DirectoryCapabilities {
  acceptMove: boolean;
  atomicReplace: boolean;
}

export interface BreadcrumbSegment {
  label: string;
  directory: DirectoryRef;
}

export interface LocationSummary {
  id: string;
  name: string;
  kind: LocationKind;
  role: LocationRole;
  status: LocationStatus;
  displayPath: string;
  detail: string;
  root: DirectoryRef;
}

export interface VolumeSnapshot {
  revision: number;
  volumes: readonly LocationSummary[];
  warning: string | null;
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

export interface EntryCapabilities {
  rename: boolean;
  move: boolean;
  trash: boolean;
  deletePermanently: boolean;
}

export interface FileEntrySummary {
  reference: EntryRef;
  name: string;
  kind: EntryKind;
  contentKind: ContentKind;
  size: string | null;
  modifiedAt: number | null;
  displayPath: string;
  directory: DirectoryRef | null;
  detail?: string;
  capabilities: EntryCapabilities;
}

export type FileOperationPrompt =
  | {
      id: string;
      kind: "permanentDelete";
      title: string;
      message: string;
      targetName: string;
      locationName: string;
      confirmLabel: string;
    }
  | {
      id: string;
      kind: "moveConflict";
      title: string;
      message: string;
      targetName: string;
      destinationName: string;
      decisions: readonly Extract<
        FileOperationPromptResponse,
        "keepBoth" | "skip" | "cancel"
      >[];
    };

export type FileOperationConfirmation = Extract<
  FileOperationPrompt,
  { kind: "permanentDelete" }
>;

export type FileOperationPromptResponse =
  "confirm" | "keepBoth" | "skip" | "cancel";

export type FileMoveResult =
  | {
      kind: "moved";
      entry: FileEntrySummary;
      sourceParent: DirectoryRef;
      destination: DirectoryRef;
      rebasedEntryIds: readonly string[];
    }
  | {
      kind: "moveSkipped";
      entry: EntryRef;
      name: string;
    };

export interface FileRemovalResult {
  kind: "trashed" | "deletedPermanently";
  entry: EntryRef;
  name: string;
  invalidatedEntryIds: readonly string[];
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
  | "remote"
  | "directory"
  | "symlink"
  | "tooLarge"
  | "binary"
  | "malformed"
  | "timedOut";

export type PreviewContent =
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
