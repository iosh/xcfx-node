import { type ChildProcess, fork } from "node:child_process";
import path from "node:path";
import type { ConfluxConfig } from "../conflux";
import { buildConfig } from "./configs";
import { waitForNodeRPCReady } from "./sync";
import {
  type Config,
  DEFAULT_CONFIG,
  type MessageFromWorker,
  type MessageToWorker,
  type NodeRequestOptions,
  type WorkerEvents,
} from "./types";

export class ConfluxInstance {
  private isServiceStarted = false;
  private readonly config: ConfluxConfig;
  private readonly timeout: number;
  private readonly retryInterval: number;
  private worker: ChildProcess | null = null;
  private events: WorkerEvents;
  private readonly stopTimeout: number;

  constructor(config: Config) {
    this.timeout = config.timeout || DEFAULT_CONFIG.timeout;
    this.retryInterval = config.retryInterval || DEFAULT_CONFIG.retryInterval;
    this.stopTimeout = config.timeout || DEFAULT_CONFIG.timeout;

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
        "This instance has already been started, you can't start it again",
      );
    }

    return new Promise<void>((resolve, reject) => {
      const workerPath = path.join(__dirname, "./worker.js");
      this.worker = fork(workerPath, {
        stdio: ["ignore", "pipe", "pipe", "ipc"],
      });

      // Always drain stdout/stderr so the worker can't block on a full pipe.
      this.worker.stdout?.resume();
      this.worker.stderr?.resume();

      let settled = false;
      const cleanupStartListeners = () => {
        if (!this.worker) return;
        this.worker.removeListener("message", handleStartMessage);
        this.worker.removeListener("error", handleError);
        // Keep the exit handler installed so we still cleanup if the worker exits later.
      };

      const finish = (error?: Error) => {
        if (settled) return;
        settled = true;
        cleanupStartListeners();
        if (error) {
          reject(error);
        } else {
          resolve();
        }
      };

      const startMessage: MessageToWorker = {
        type: "start",
        config: this.config,
      };

      const handleStartMessage = (message: MessageFromWorker) => {
        if (message.type === "started") {
          this.events.onStart?.();
          this.waitForRPCReady()
            .then(() => finish())
            .catch(finish);
          return;
        }
        if (message.type === "error") {
          const error = new Error(message.error);
          if (message.stack) {
            error.stack = message.stack;
          }
          this.events.onError?.(error);
          finish(error);
        }
      };

      const handleError = (error: Error) => {
        this.events.onError?.(error);
        finish(error);
      };

      const handleExit = (code: number | null, signal: string | null) => {
        this.events.onExit?.(code, signal);
        finish(
          new Error(
            code === null
              ? `Worker process exited with signal ${signal ?? "unknown"}`
              : `Worker process exited with code ${code}`,
          ),
        );
      };

      this.worker.on("message", handleStartMessage);
      this.worker.on("error", handleError);
      this.worker.on("exit", handleExit);

      this.worker.send(startMessage);
    });
  };

  stop = async () => {
    if (!this.worker || !this.isServiceStarted) {
      throw new Error(
        "This instance has not been started or is already stopped",
      );
    }

    const worker = this.worker;

    return new Promise<void>((resolve, reject) => {
      let settled = false;
      let stopped = false;
      const stopTimer = setTimeout(() => {
        if (settled) return;
        if (!worker.killed && worker.exitCode === null) {
          worker.kill();
        }
      }, this.stopTimeout);

      const cleanupStopListeners = () => {
        clearTimeout(stopTimer);
        worker.removeListener("message", handleMessage);
        worker.removeListener("error", handleError);
        worker.removeListener("exit", handleExit);
      };

      const finish = (error?: Error) => {
        if (settled) return;
        settled = true;
        cleanupStopListeners();
        if (error) {
          reject(error);
        } else {
          resolve();
        }
      };

      const handleMessage = (message: MessageFromWorker) => {
        if (message.type === "stopped") {
          stopped = true;
          this.events.onStop?.();
          return;
        }
        if (message.type === "error") {
          const error = new Error(message.error);
          this.events.onError?.(error);
          finish(error);
        }
      };

      const handleError = (error: Error) => {
        this.events.onError?.(error);
        finish(error);
      };

      const handleExit = (code: number | null, signal: string | null) => {
        if (!stopped) {
          this.events.onStop?.();
        }
        this.events.onExit?.(code, signal);
        this.cleanup();
        if (code !== 0 && code !== null) {
          finish(new Error(`Worker process exited with code ${code}`));
          return;
        }
        finish();
      };

      if (worker.exitCode !== null) {
        handleExit(worker.exitCode, null);
        return;
      }

      worker.on("message", handleMessage);
      worker.on("error", handleError);
      worker.on("exit", handleExit);
      worker.send({ type: "stop" } as MessageToWorker);
    });
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
