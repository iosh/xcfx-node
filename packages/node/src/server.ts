import path from "path";
import { onExit } from "signal-exit";
import { cleanup, getBinPath } from "./helper";
import { ConfluxConfig, ServerConfig } from "./types";
import { ChildProcessWithoutNullStreams, spawn } from "child_process";
import { CONFIG_FILE_NAME } from "./createConfigFile";

export class ConfluxServer {
  config: ConfluxConfig & ServerConfig;
  conflux: ChildProcessWithoutNullStreams | null = null;
  workDir: string;

  constructor(config: ConfluxConfig & ServerConfig = {}) {
    this.config = {
      ...config,
      waitUntilReady: config.waitUntilReady || true,
      silent: config.silent || true,
      persistNodeData: config.persistNodeData || false,
    };

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

    return new Promise<void>((resolve, reject) => {
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
        }
      );

      this.conflux.stderr.on("data", (msg) => {
        console.log(`Conflux node error: ${msg.toString()}`);
        this.syncStop()
        reject(msg.toString());
      });

      if (!this.config.silent) {
        this.conflux.stdout.on("data", (msg) => {
          console.log(msg.toString());
        });
      }

      if (this.config.waitUntilReady) {
        this.conflux.stdout.on("data", (msg) => {
          const msgStr = msg.toString();
          if (msgStr.includes("Conflux client started")) {
            resolve();
          }
        });
      } else {
        resolve();
      }
    });
  }

  syncStop () {
    if (!this.conflux) return;
    if (this.conflux) {
      this.conflux.stdout.destroy();
      this.conflux = null;
    }
    if (!this.config.persistNodeData) {
      console.log("cleaning up conflux node data...");
      cleanup(this.workDir);
    }
  }

  /**
   * Stop the conflux server
   */
  async stop() {
    this.syncStop()
  }
}
