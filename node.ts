import { ConfluxNode, type ConfluxConfig } from "./conflux";

class NodeManager {
  private node: ConfluxNode | null = null;
  private isStarted: boolean = false;

  constructor() {
    this.setupMessageHandlers();
    this.setupErrorHandlers();
  }

  private setupMessageHandlers = () => {
    process.on("message", this.handleMessage);
  };

  private setupErrorHandlers = () => {
    process.on("uncaughtException", this.handleError);
  };

  private handleMessage = (message: {
    type: string;
    config?: ConfluxConfig;
  }) => {
    if (!process.send) return;

    switch (message.type) {
      case "start":
        this.handleStart(message.config);
        break;
      case "stop":
        this.handleStop();
        break;
      default:
        this.sendError(`Unknown message type: ${message.type}`);
    }
  };

  private handleStart = (config?: ConfluxConfig) => {
    if (!config) {
      this.sendError("No configuration provided for start");
      return;
    }

    if (this.isStarted) {
      this.sendError("Node is already started");
      return;
    }

    if (!this.node) {
      this.node = new ConfluxNode();
    }

    this.node.startNode(config, (err) => {
      if (err) {
        this.sendError(err.message);
      } else {
        this.isStarted = true;
        this.sendMessage("started");
      }
    });
  };

  private handleStop = () => {
    if (!this.node) {
      this.sendMessage("stopped");
      this.exit(0);
      return;
    }

    this.node.stopNode((err) => {
      if (err) {
        this.sendError(err.message);
      } else {
        this.isStarted = false;
        this.node = null;
        this.sendMessage("stopped");
        this.exit(0);
      }
    });
  };

  private handleError = (err: Error) => {
    this.sendError(err.message);
    this.exit(1);
  };

  private sendMessage = (type: string) => {
    process.send?.({ type });
  };

  private sendError = (error: string) => {
    process.send?.({ type: "error", error });
  };

  private exit = (code: number) => {
    process.exit(code);
  };
}

new NodeManager();
