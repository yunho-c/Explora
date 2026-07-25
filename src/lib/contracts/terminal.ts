export type TerminalSessionState =
  "starting" | "running" | "exited" | "failed" | "closing";

export type TerminalSessionKind = "local" | "ssh";
export type TerminalCloseReason =
  "user" | "restart" | "windowClosed" | "applicationExit" | "channelClosed";
export type TerminalExitReason = "completed" | "terminated" | "transportClosed";

export interface TerminalSize {
  columns: number;
  rows: number;
  pixelWidth: number | null;
  pixelHeight: number | null;
}

export interface TerminalSessionSummary {
  id: string;
  state: TerminalSessionState;
  kind: TerminalSessionKind;
  locationId: string;
  title: string;
  contextLabel: string;
}

export interface TerminalError {
  code:
    | "invalidReference"
    | "notFound"
    | "permissionDenied"
    | "notDirectory"
    | "cancelled"
    | "offline"
    | "authenticationFailed"
    | "hostKeyFailure"
    | "unsupported"
    | "invalidConfiguration"
    | "unexpected";
  message: string;
}

export type TerminalEvent =
  | { event: "started"; session: TerminalSessionSummary }
  | { event: "output"; sequence: number; bytes: Uint8Array }
  | {
      event: "exited";
      exitCode: number | null;
      signal: string | null;
      reason: TerminalExitReason;
    }
  | { event: "failed"; error: TerminalError };

export interface TerminalLaunchContext {
  locationId: string;
  directoryId: string | null;
  kind: TerminalSessionKind;
  locationLabel: string;
  directoryLabel: string;
}
