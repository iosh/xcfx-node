import { fork, type ChildProcess } from "node:child_process";
import path from "node:path";
import type { ConfluxConfig } from "../conflux";
import { buildConfig } from "./configs";
import { waitForNodeRPCReady } from "./sync";
import {
  type Config,
  DEFAULT_CONFIG,
  type NodeRequestOptions,
  type MessageToWorker,
  type MessageFromWorker,
  type WorkerEvents,
} from "./types";

export class ConfluxInstance {
  private isServiceStarted = false;
  private readonly config: ConfluxConfig;
  private readonly timeout: number;
  private readonly retryInterval: number;
  private worker: ChildProcess | null = null;
  private events: WorkerEvents;

  constructor(config: Config) {
    this.timeout = config.timeout || DEFAULT_CONFIG.timeout;
    this.retryInterval = config.retryInterval || DEFAULT_CONFIG.retryInterval;

    this.config = buildConfig(config);

    this.events = {
      onStart: () => {
        this.isServiceStarted = true;
      },
      onStop: () => {
        this.isServiceStarted = false;
      },
      onError: (error: Error) => {
        console.error("Conflux node error:", error);
      },
      onExit: (_code: number | null, _signal: string | null) => {
        this.cleanup();
      },
    };
  }

  start = async () => {
    if (this.isServiceStarted) {
      throw new Error(
        "This instance has already been started, you can't start it again"
      );
    }

    return new Promise<void>((resolve, reject) => {
      const workerPath = path.join(__dirname, "./worker.js");
      this.worker = fork(workerPath);

      this.setupWorkerListeners(resolve, reject);

      const startMessage: MessageToWorker = {
        type: "start",
        config: this.config,
      };
      this.worker.send(startMessage);
    });
  };

  stop = async () => {
    if (!this.worker || !this.isServiceStarted) {
      throw new Error(
        "This instance has not been started or is already stopped"
      );
    }

    return new Promise<void>((resolve, reject) => {
      if (!this.worker) {
        return;
      }

      const handleMessage = (message: MessageFromWorker) => {
        if (message.type === "stopped") {
          this.events.onStop?.();
          resolve();
        } else if (message.type === "error") {
          const error = new Error(message.error);
          this.events.onError?.(error);
          reject(error);
        }
      };

      const handleExit = (code: number | null, signal: string | null) => {
        this.events.onExit?.(code, signal);
        this.cleanup();
        resolve();
      };

      this.worker.on("message", handleMessage);
      this.worker.on("exit", handleExit);
      this.worker.send({ type: "stop" } as MessageToWorker);
    });
  };

  private setupWorkerListeners = (
    resolve: () => void,
    reject: (error: Error) => void
  ) => {
    if (!this.worker) return;

    const handleStartMessage = (message: MessageFromWorker) => {
      if (message.type === "started") {
        this.events.onStart?.();
        this.waitForRPCReady().then(resolve).catch(reject);
      } else if (message.type === "error") {
        const error = new Error(message.error);
        if (message.stack) {
          error.stack = message.stack;
        }
        this.events.onError?.(error);
        reject(error);
      }
    };

    const handleError = (error: Error) => {
      this.events.onError?.(error);
      reject(error);
    };

    const handleExit = (code: number | null, signal: string | null) => {
      this.events.onExit?.(code, signal);
      this.cleanup();
      if (code !== 0 && code !== null) {
        reject(new Error(`Worker process exited with code ${code}`));
      }
    };

    this.worker.on("message", handleStartMessage);
    this.worker.on("error", handleError);
    this.worker.on("exit", handleExit);
  };

  private waitForRPCReady = async (): Promise<void> => {
    if (!this.config.jsonrpcHttpPort && !this.config.jsonrpcWsPort) {
      return;
    }

    const rpcConfig: NodeRequestOptions = {
      httpPort: this.config.jsonrpcHttpPort,
      wsPort: this.config.jsonrpcWsPort,
      timeout: this.timeout,
      retryInterval: this.retryInterval,
    };

    await waitForNodeRPCReady(rpcConfig);
  };

  private cleanup = () => {
    if (this.worker) {
      this.worker.removeAllListeners();
      if (!this.worker.killed) {
        this.worker.kill();
      }
      this.worker = null;
    }
    this.isServiceStarted = false;
  };
}
