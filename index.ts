import path from "node:path";
import type { ConfluxConfig } from "./conflux";
import { Config, DEFAULT_CONFIG, NodeRequestOptions } from "./lib/types";
import { ProcessManager } from "./lib/process/manager";
import { waitForNodeRPCReady } from "./lib/node/sync";

export interface CreateServerReturnType {
  start: () => Promise<void>;
  stop: () => Promise<void>;
}
class ConfluxInstance {
  private isServiceStarted = false;
  private readonly config: ConfluxConfig;
  private readonly timeout: number;
  private readonly retryInterval: number;
  private processManager: ProcessManager;

  constructor(config: Config) {
    const finalConfig = { ...DEFAULT_CONFIG, ...config };
    this.timeout = finalConfig.timeout;
    this.retryInterval = finalConfig.retryInterval;

    // Build base configuration
    this.config = {
      posConfigPath: path.join(
        __dirname,
        "./configs/pos_config/pos_config.yaml"
      ),
      posInitialNodesPath: path.join(
        __dirname,
        "./configs/pos_config/initial_nodes.json"
      ),
      logConf: finalConfig.log
        ? path.join(__dirname, "./configs/log.yaml")
        : undefined,
      ...config,
    };

    this.processManager = new ProcessManager({
      onStart: () => {
        this.isServiceStarted = true;
      },
      onStop: () => {
        this.isServiceStarted = false;
      },
    });
  }

  /**
   * Starts the Conflux node instance
   * @throws {Error} If the instance is already started
   */
  start = async () => {
    if (this.isServiceStarted) {
      throw new Error(
        "This instance has already been started, you can't start it again"
      );
    }

    await this.processManager.start(this.config);
    // Wait for node synchronization if RPC ports are configured
    if (this.config.jsonrpcHttpPort || this.config.jsonrpcWsPort) {
      await this.waitForSyncPhase();
    }

    this.isServiceStarted = true;
  };

  /**
   * Stops the Conflux node instance
   * @returns Promise that resolves when the node is stopped
   */
  async stop(): Promise<void> {
    await this.processManager.stop();
  }

  /**
   * Waits for the node to reach normal sync phase
   * @throws {Error} If sync phase check times out
   */
  private waitForSyncPhase = async () => {
    const config: NodeRequestOptions = {
      httpPort: this.config.jsonrpcHttpPort,
      wsPort: this.config.jsonrpcWsPort,
      timeout: this.timeout,
      retryInterval: this.retryInterval,
    };

    await waitForNodeRPCReady(config);
  };
}

/**
 * Creates a new Conflux server instance
 * @param config - Server configuration options
 * @returns Object with start and stop methods
 */
export const createServer = async (
  config: Config = {}
): Promise<CreateServerReturnType> => {
  const instance = new ConfluxInstance(config);
  return {
    start: () => instance.start(),
    stop: () => instance.stop(),
  };
};
