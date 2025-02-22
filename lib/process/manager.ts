import { fork, type ChildProcess } from "node:child_process";
import path from "node:path";
import { NodeMessage, ProcessEvents, PromiseHandlers } from "../types";
import { ConfluxConfig } from "../../conflux";

export class ProcessManager {
  private process: ChildProcess | null = null;
  private isRunning = false;

  constructor(private readonly events: ProcessEvents = {}) {}

  start = async (config: ConfluxConfig) => {
    return new Promise<void>((resolve, reject) => {
      this.process = fork(path.join(__dirname, "../node/node.js"));
      this.setupListeners({ resolve, reject });
      this.process.send({ type: "start", config });
    });
  };

  setupListeners = (handle: PromiseHandlers) => {
    if (!this.process) return;

    this.process.on("message", (message: NodeMessage) => {
      if (message.type === "started") {
        this.events.onStart?.();
        handle.resolve();
        this.isRunning = true;
        this.cleanup();
      }
      if (message.type === "error") {
        this.events.onError?.(new Error(message.error));
        handle.reject(new Error(message.error));
      }
    });

    this.process.on("error", (error) => {
      this.events.onError?.(error);
      handle.reject(error);
    });

    this.process.on("exit", (code) => {
      if (code !== 0 && !this.isRunning) {
        const error = new Error(`Worker process exited with code ${code}`);
        handle.reject(error);
        this.events.onError?.(error);
      }

      this.events.onExit?.(code || 0);

      this.isRunning = false;

      this.process = null;
    });
  };

  cleanup = async () => {
    const _cleanup = () => {
      if (!this.process) return;
      this.process.kill("SIGKILL");
    };

    process.once("exit", _cleanup);
    process.once("SIGINT", _cleanup);
    process.once("SIGTERM", _cleanup);
  };

  stop = async () => {
    if (!this.process) return;

    return new Promise<void>((resolve, reject) => {
      this.setupStopListeners({ resolve, reject });
      this.process?.send({ type: "stop" });
    });
  };

  setupStopListeners = (handle: PromiseHandlers) => {
    if (!this.process) {
      handle.resolve();
      return;
    }

    this.process.once("message", (message: NodeMessage) => {
      if (message.type === "error") {
        handle.reject(new Error(message.error));
        this.events.onError?.(new Error(message.error));
        return;
      }
      if (message.type === "stopped") {
        this.process = null;
        this.isRunning = false;
        handle.resolve();
        this.events.onStop?.();
      }
    });
  };
}
