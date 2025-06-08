import type { ConfluxConfig } from "../conflux";

export interface Config extends ConfluxConfig {
  /** Whether to show conflux node logs */
  log?: boolean;
  /** Timeout in milliseconds */
  timeout?: number;
  /** Retry interval in milliseconds */
  retryInterval?: number;
}

export interface NodeRequestOptions {
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

export interface StartWorkerMessage {
  type: "start";
  config: ConfluxConfig;
}

export interface StopWorkerMessage {
  type: "stop";
}

export type MessageToWorker = StartWorkerMessage | StopWorkerMessage;

export interface StartedMainMessage {
  type: "started";
}

export interface StoppedMainMessage {
  type: "stopped";
}

export interface ErrorMainMessage {
  type: "error";
  error: string;
  stack?: string;
}

export type MessageFromWorker =
  | StartedMainMessage
  | StoppedMainMessage
  | ErrorMainMessage;

// Worker 事件回调
export interface WorkerEvents {
  onStart?: () => void;
  onStop?: () => void;
  onError?: (error: Error) => void;
  onExit?: (code: number | null, signal: string | null) => void;
}
