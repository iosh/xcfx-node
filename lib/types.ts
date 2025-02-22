import { ConfluxConfig } from "../conflux";

export interface Config extends ConfluxConfig {
  /** Whether to show conflux node logs */
  log?: boolean;
  /** Timeout in milliseconds */
  timeout?: number;
  /** Retry interval in milliseconds */
  retryInterval?: number;
}

export interface SyncPhaseConfig {
  httpPort?: number;
  wsPort?: number;
  timeout: number;
  retryInterval: number;
}

// Default configuration
export const DEFAULT_CONFIG = {
  timeout: 20000,
  retryInterval: 300,
  log: false,
};

export type MessageType = "start" | "started" | "stop" | "stopped" | "error";

export interface NodeMessage {
  type: MessageType;
  error?: string;
  config?: ConfluxConfig;
}

export interface ProcessEvents {
  onStart?: () => void;
  onStop?: () => void;
  onError?: (error: Error) => void;
  onExit?: (code: number) => void;
}

export interface PromiseHandlers {
  resolve: () => void;
  reject: (error: Error) => void;
}
