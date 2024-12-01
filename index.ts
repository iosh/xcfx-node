import path from "node:path";
import { http, createTestClient, webSocket } from "cive";
import { type ConfluxConfig, ConfluxNode } from "./conflux";

export type Config = {
  /**
   * show conflux node log
   * @default false
   */
  log?: boolean;
  timeout?: number;
  retryInterval?: number;
} & ConfluxConfig;

export type CreateServerReturnType = {
  start: () => Promise<void>;
  stop: () => Promise<void>;
};

let isServiceCreated = false;
export async function createServer(
  config: Config = {},
): Promise<CreateServerReturnType> {
  if (isServiceCreated) {
    throw new Error("The server has already been created");
  }
  const { timeout = 20000, retryInterval = 300 } = config;

  isServiceCreated = true;

  const { log = false, ...userConfig } = config;

  const filledConfig: ConfluxConfig = {
    posConfigPath: path.join(__dirname, "./configs/pos_config/pos_config.yaml"),
    posInitialNodesPath: path.join(
      __dirname,
      "./configs/pos_config/initial_nodes.json",
    ),
    posPrivateKeyPath: path.join(__dirname, "../configs/pos_config/pos_key"),

    logConf: log ? path.join(__dirname, "./configs/log.yaml") : undefined,

    ...userConfig,
  };

  const node = new ConfluxNode();
  return {
    async start() {
      await new Promise<void>((resolve, reject) => {
        node.startNode(filledConfig, (err) => {
          if (err) {
            reject(err);
          } else {
            resolve();
          }
        });
      });
      console.log("node start", new Date());
      if (filledConfig.jsonrpcHttpPort || filledConfig.jsonrpcWsPort) {
        await retryGetCurrentSyncPhase({
          httpPort: filledConfig.jsonrpcHttpPort,
          wsPort: filledConfig.jsonrpcWsPort,
          timeout: timeout,
          retryInterval: retryInterval,
        });
      }
    },
    async stop() {
      return new Promise((resolve, reject) => {
        node.stopNode((err) => {
          if (err) {
            reject(err);
          } else {
            resolve();
          }
        });
      });
    },
  };
}

type retryGetCurrentSyncPhaseParameters = {
  httpPort?: number;
  wsPort?: number;
  timeout: number;
  retryInterval: number;
};

async function retryGetCurrentSyncPhase({
  httpPort,
  wsPort,
  timeout,
  retryInterval,
}: retryGetCurrentSyncPhaseParameters) {
  if (!httpPort && !wsPort) return;

  const testClient = createTestClient({
    transport: httpPort
      ? http(`http://127.0.0.1:${httpPort}`)
      : webSocket(`ws://127.0.0.1:${wsPort}`),
  });

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
}
