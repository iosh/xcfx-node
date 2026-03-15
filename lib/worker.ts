import { type ConfluxConfig, ConfluxNode } from "../conflux";
import type { MessageFromWorker, MessageToWorker } from "./types";

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

const isStartMessage = (
  message: unknown,
): message is Extract<MessageToWorker, { type: "start" }> =>
  isObject(message) && message.type === "start" && "config" in message;

const isStopMessage = (
  message: unknown,
): message is Extract<MessageToWorker, { type: "stop" }> =>
  isObject(message) && message.type === "stop";

class WorkerManager {
  private node: ConfluxNode | null = null;
  private isStarted = false;

  constructor() {
    this.setupMessageHandlers();
    this.setupErrorHandlers();
    this.setupSignalHandlers();
  }

  private setupMessageHandlers = () => {
    process.on("message", this.handleMessage);
  };

  private setupErrorHandlers = () => {
    process.on("uncaughtException", (error) => this.handleError(error));
    process.on("unhandledRejection", (reason) => this.handleError(reason));
  };

  private setupSignalHandlers = () => {
    process.on("SIGTERM", () => {
      this.exit(0);
    });

    process.on("SIGINT", () => {
      this.exit(0);
    });
  };

  private handleMessage = (message: unknown) => {
    if (!process.send) return;

    if (isStartMessage(message)) {
      this.handleStart(message.config);
      return;
    }

    if (isStopMessage(message)) {
      this.handleStop();
      return;
    }

    const messageType =
      isObject(message) && "type" in message ? String(message.type) : "unknown";
    this.sendError(`Unknown message type: ${messageType}`);
  };

  private handleStart = async (config: ConfluxConfig) => {
    if (this.isStarted) {
      this.sendError("Node is already started");
      return;
    }

    try {
      if (!this.node) {
        this.node = new ConfluxNode();
      }

      await this.node.startNode(config);
      this.isStarted = true;
      this.sendMessage("started");
    } catch (error) {
      this.sendError(error instanceof Error ? error.message : String(error));
    }
  };

  private handleStop = async () => {
    try {
      if (!this.node) {
        this.sendMessage("stopped");
        this.exit(0);
        return;
      }

      await this.node.stopNode();
      this.isStarted = false;
      this.node = null;
      this.sendMessage("stopped");
      this.exit(0);
    } catch (error) {
      this.sendError(error instanceof Error ? error.message : String(error));
      this.exit(1);
    }
  };

  private handleError = (error: unknown) => {
    const normalizedError =
      error instanceof Error ? error : new Error(String(error));
    this.sendError(normalizedError.message, normalizedError.stack);
    this.exit(1);
  };

  private sendMessage = (type: "started" | "stopped") => {
    const message: MessageFromWorker =
      type === "started" ? { type: "started" } : { type: "stopped" };
    process.send?.(message);
  };

  private sendError = (error: string, stack?: string) => {
    const message: MessageFromWorker = {
      type: "error",
      error,
      ...(stack && { stack }),
    };
    process.send?.(message);
  };

  private exit = (code: number) => {
    process.exit(code);
  };
}

new WorkerManager();
