import path from "path";
import { onExit } from "signal-exit";
import { cleanup, getBinPath, waitConfluxNodeReady } from "./helper";
import { ServerConfig } from "./types";
import { ChildProcessWithoutNullStreams, spawn } from "child_process";
import { CONFIG_FILE_NAME, FinalConfigs } from "./createConfigFile";

export class ConfluxServer {
  config: FinalConfigs & ServerConfig;
  conflux: ChildProcessWithoutNullStreams | null = null;
  workDir: string;

  constructor(config: FinalConfigs & ServerConfig) {
    this.config = config;

    this.workDir = path.join(__dirname, "../data");

    onExit(this.syncStop.bind(this));
  }

  /**
   * Start the conflux node
   * @returns {Promise<void>}
   */
  async start() {
    const binPathResult = getBinPath();

    if ("errorMessage" in binPathResult) {
      throw new Error(binPathResult.errorMessage);
    }

    this.conflux = spawn(
      binPathResult.binPath,
      [
        "--config",
        path.join(this.workDir, CONFIG_FILE_NAME),
        "--block-db-dir",
        path.join(this.workDir, "db"),
      ],
      {
        cwd: this.workDir,
      },
    );

    this.conflux.stderr.on("data", this.onListenError);

    if (!this.config.silent) {
      this.conflux.stdout.on("data", this.onListenData);
    }

    if (this.config.waitUntilReady) {
      await waitConfluxNodeReady({
        chainId: this.config.chain_id,
        httpPort: this.config.jsonrpc_http_port,
      });
    }
  }

  syncStop() {
    if (!this.conflux) return;
    if (this.conflux) {
      this.conflux.stdout.destroy();
      this.conflux = null;
    }
    if (!this.config.persistNodeData) {
      cleanup(this.workDir);
    }
  }

  /**
   * Stop the conflux server
   */
  async stop() {
    this.syncStop();
  }

  onListenError = (chunk: any) => {
    const msg = chunk.toString();
    console.error(`Conflux node error: ${msg.toString()}`);
  };

  onListenData = (chunk: any) => {
    const msg = chunk.toString();
    console.log(msg.toString());
  };
}
