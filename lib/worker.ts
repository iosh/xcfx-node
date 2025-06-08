import { ConfluxNode, type ConfluxConfig } from "../conflux";
import type { MessageToWorker, MessageFromWorker } from "./types";

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
    process.on("uncaughtException", this.handleError);
    process.on("unhandledRejection", this.handleError);
  };

  private setupSignalHandlers = () => {
    process.on("SIGTERM", () => {
      this.exit(0);
    });

    process.on("SIGINT", () => {
      this.exit(0);
    });
  };

  private handleMessage = (message: MessageToWorker) => {
    if (!process.send) return;

    switch (message.type) {
      case "start":
        this.handleStart(message.config);
        break;
      case "stop":
        this.handleStop();
        break;
      default:
        this.sendError(`Unknown message type: ${(message as any).type}`);
    }
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

  private handleError = (error: Error) => {
    this.sendError(error.message, error.stack);
    this.exit(1);
  };

  private sendMessage = (type: MessageFromWorker["type"], error = "") => {
    const message: MessageFromWorker = { type, error };
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
