import path from "node:path";
import { fork, type ChildProcess } from "node:child_process";
import { http, createTestClient, webSocket } from "cive";
import { type ConfluxConfig } from "./conflux";

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

class ConfluxInstance {
  private nodeProcess: ChildProcess | null = null;
  private isServiceStarted = false;
  private readonly config: ConfluxConfig;
  private readonly timeout: number;
  private readonly retryInterval: number;

  constructor(config: Config) {
    const {
      timeout = 20000,
      retryInterval = 300,
      log = false,
      ...userConfig
    } = config;

    this.timeout = timeout;
    this.retryInterval = retryInterval;
    this.config = {
      posConfigPath: path.join(
        __dirname,
        "./configs/pos_config/pos_config.yaml"
      ),
      posInitialNodesPath: path.join(
        __dirname,
        "./configs/pos_config/initial_nodes.json"
      ),
      logConf: log ? path.join(__dirname, "./configs/log.yaml") : undefined,
      ...userConfig,
    };
  }

  async start(): Promise<void> {
    if (this.isServiceStarted) {
      throw new Error(
        "This instance has already been started, you can't start it again"
      );
    }

    await new Promise<void>((resolve, reject) => {
      this.nodeProcess = fork(path.join(__dirname, "node.js"));

      this.nodeProcess.on(
        "message",
        (message: { type: string; error?: string }) => {
          if (message.type === "started") {
            resolve();
          } else if (message.type === "error") {
            reject(new Error(message.error));
          }
        }
      );

      this.nodeProcess.on("error", (err) => {
        reject(err);
      });

      this.nodeProcess.on("exit", (code) => {
        if (code !== 0 && !this.isServiceStarted) {
          reject(new Error(`Worker process exited with code ${code}`));
        }
        this.isServiceStarted = false;
        this.nodeProcess = null;
      });

      this.nodeProcess.send({ type: "start", config: this.config });
    });

    if (this.config.jsonrpcHttpPort || this.config.jsonrpcWsPort) {
      await retryGetCurrentSyncPhase({
        httpPort: this.config.jsonrpcHttpPort,
        wsPort: this.config.jsonrpcWsPort,
        timeout: this.timeout,
        retryInterval: this.retryInterval,
      });
    }
    this.isServiceStarted = true;
  }

  async stop(): Promise<void> {
    if (!this.nodeProcess) {
      return;
    }

    return new Promise<void>((resolve, reject) => {
      if (!this.nodeProcess) {
        resolve();
        return;
      }

      this.nodeProcess.on(
        "message",
        (message: { type: string; error?: string }) => {
          if (message.type === "stopped") {
            this.nodeProcess = null;
            this.isServiceStarted = false;
            resolve();
          } else if (message.type === "error") {
            reject(new Error(message.error));
          }
        }
      );

      this.nodeProcess.send({ type: "stop" });
    });
  }
}

export async function createServer(
  config: Config = {}
): Promise<CreateServerReturnType> {
  const instance = new ConfluxInstance(config);
  return {
    start: () => instance.start(),
    stop: () => instance.stop(),
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
