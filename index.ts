import path from "node:path";
import { fork, type ChildProcess } from "node:child_process";
import { http, createTestClient, webSocket } from "cive";
import type { ConfluxConfig } from "./conflux";
// Type definitions
export interface Config extends ConfluxConfig {
  /** Whether to show conflux node logs */
  log?: boolean;
  /** Timeout in milliseconds */
  timeout?: number;
  /** Retry interval in milliseconds */
  retryInterval?: number;
}

export interface CreateServerReturnType {
  start: () => Promise<void>;
  stop: () => Promise<void>;
}

interface NodeMessage {
  type: string;
  error?: string;
}

interface SyncPhaseConfig {
  httpPort?: number;
  wsPort?: number;
  timeout: number;
  retryInterval: number;
}

// Default configuration
const DEFAULT_CONFIG = {
  timeout: 20000,
  retryInterval: 300,
  log: false,
};

class ConfluxInstance {
  private nodeProcess: ChildProcess | null = null;
  private isServiceStarted = false;
  private readonly config: ConfluxConfig;
  private readonly timeout: number;
  private readonly retryInterval: number;

  constructor(config: Config) {
    const finalConfig = { ...DEFAULT_CONFIG, ...config };
    this.timeout = finalConfig.timeout;
    this.retryInterval = finalConfig.retryInterval;

    // Build base configuration
    this.config = {
      posConfigPath: path.join(
        __dirname,
        "./configs/pos_config/pos_config.yaml",
      ),
      posInitialNodesPath: path.join(
        __dirname,
        "./configs/pos_config/initial_nodes.json",
      ),
      logConf: finalConfig.log
        ? path.join(__dirname, "./configs/log.yaml")
        : undefined,
      ...config,
    };
  }

  private setupProcessListeners = (
    resolve: () => void,
    reject: (error: Error) => void,
  ) => {
    if (!this.nodeProcess) return;

    const handleMessage = (message: NodeMessage) => {
      switch (message.type) {
        case "started":
          resolve();
          break;
        case "error":
          reject(new Error(message.error));
          break;
      }
    };

    const handleError = (err: Error) => {
      reject(err);
    };

    const handleExit = (code: number) => {
      if (code !== 0 && !this.isServiceStarted) {
        reject(new Error(`Worker process exited with code ${code}`));
      }
      this.isServiceStarted = false;
      this.nodeProcess = null;
    };

    this.nodeProcess.on("message", handleMessage);
    this.nodeProcess.on("error", handleError);
    this.nodeProcess.on("exit", handleExit);
  };

  private setupStopListeners = (
    resolve: () => void,
    reject: (error: Error) => void,
  ) => {
    if (!this.nodeProcess) {
      resolve();
      return;
    }

    const handleMessage = (message: NodeMessage) => {
      switch (message.type) {
        case "stopped":
          this.nodeProcess = null;
          this.isServiceStarted = false;
          resolve();
          break;
        case "error":
          reject(new Error(message.error));
          break;
      }
    };

    this.nodeProcess.on("message", handleMessage);
  };

  /**
   * Starts the Conflux node instance
   * @throws {Error} If the instance is already started
   */
  async start(): Promise<void> {
    if (this.isServiceStarted) {
      throw new Error(
        "This instance has already been started, you can't start it again",
      );
    }

    await new Promise<void>((resolve, reject) => {
      this.nodeProcess = fork(path.join(__dirname, "node.js"));
      this.setupProcessListeners(resolve, reject);
      this.nodeProcess.send({ type: "start", config: this.config });

      process.once("exit", this.killProcess);
      process.once("SIGINT", this.killProcess);
      process.once("SIGTERM", this.killProcess);
    });

    // Wait for node synchronization if RPC ports are configured
    if (this.config.jsonrpcHttpPort || this.config.jsonrpcWsPort) {
      await this.waitForSyncPhase();
    }

    this.isServiceStarted = true;
  }

  /**
   * Stops the Conflux node instance
   * @returns Promise that resolves when the node is stopped
   */
  async stop(): Promise<void> {
    if (!this.nodeProcess) return;

    await new Promise<void>((resolve, reject) => {
      this.setupStopListeners(resolve, reject);
      this.nodeProcess?.send({ type: "stop" });
    });
  }

  /**
   * Waits for the node to reach normal sync phase
   * @throws {Error} If sync phase check times out
   */
  private async waitForSyncPhase(): Promise<void> {
    const config: SyncPhaseConfig = {
      httpPort: this.config.jsonrpcHttpPort,
      wsPort: this.config.jsonrpcWsPort,
      timeout: this.timeout,
      retryInterval: this.retryInterval,
    };

    await retryGetCurrentSyncPhase(config);
  }

  private killProcess = () => {
    if (!this.nodeProcess) return;
    this.nodeProcess.kill("SIGKILL");
  }
}

/**
 * Creates a new Conflux server instance
 * @param config - Server configuration options
 * @returns Object with start and stop methods
 */
export const createServer = async (
  config: Config = {},
): Promise<CreateServerReturnType> => {
  const instance = new ConfluxInstance(config);
  return {
    start: () => instance.start(),
    stop: () => instance.stop(),
  };
};

/**
 * Retries getting the current sync phase until it reaches normal state or times out
 * @param config - Sync phase configuration
 * @throws {Error} If the operation times out
 */
const retryGetCurrentSyncPhase = async ({
  httpPort,
  wsPort,
  timeout,
  retryInterval,
}: SyncPhaseConfig): Promise<void> => {
  if (!httpPort && !wsPort) return;

  const transport = httpPort
    ? http(`http://127.0.0.1:${httpPort}`)
    : webSocket(`ws://127.0.0.1:${wsPort}`);

  const testClient = createTestClient({ transport });
  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), timeout);

  try {
    while (!controller.signal.aborted) {
      const phase = await testClient.getCurrentSyncPhase();
      if (phase === "NormalSyncPhase") {
        clearTimeout(timeoutId);
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, retryInterval));
    }
  } catch (error) {
    if (controller.signal.aborted) {
      throw new Error("Get node sync phase timeout");
    }
    throw error;
  } finally {
    clearTimeout(timeoutId);
  }
};
