import { type ChildProcessWithoutNullStreams, spawn } from "node:child_process";
import path from "node:path";

import { CONFIG_FILE_NAME, createConfigFile } from "./createConfigFile";
import type { ConfluxConfig, ServerConfig } from "./types";
import { checkEnvironment, cleanup } from "./helper";

export type createServerReturnType = {
  start: () => Promise<void>;
  stop: () => Promise<void>;
};

async function createServer({
  waitUntilReady = true,
  silent = true,
  persistNodeData = false,
  ...config
}: ConfluxConfig & ServerConfig = {}): Promise<createServerReturnType> {
  const checkResult = checkEnvironment();

  if (!checkResult.supportPlatform) {
    throw new Error(checkResult.message);
  }
  const workDir = path.join(__dirname, "../data");
  // try to cleanup the data dir
  if (!persistNodeData) {
    cleanup(workDir);
  }

  await createConfigFile(config);

  let conflux: ChildProcessWithoutNullStreams | null = null;

  function stop() {
    if (!conflux) return;
    if (conflux) {
      conflux.stdout.destroy();
      conflux.kill("SIGINT");
      conflux = null;
    }
    if (!persistNodeData) {
      cleanup(workDir);
    }
  }

  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
  return {
    start: async () => {
      return new Promise<void>((resolve, reject) => {
        conflux = spawn(
          checkResult.binPath,
          [
            "--config",
            path.join(workDir, CONFIG_FILE_NAME),
            "--block-db-dir",
            path.join(workDir, "db"),
          ],
          {
            cwd: workDir,
          },
        );

        conflux.stderr.on("data", (msg) => {
          console.log(`Conflux node error: ${msg.toString()}`);
          stop();
          reject(msg.toString());
        });

        if (!silent) {
          conflux.stdout.on("data", (msg) => {
            console.log(msg.toString());
          });
        }

        if (waitUntilReady) {
          conflux.stdout.on("data", (msg) => {
            const msgStr = msg.toString();
            if (msgStr.includes("Conflux client started")) {
              resolve();
            }
          });
        } else {
          resolve();
        }
      });
    },
    stop: async () => stop(),
  };
}

export default createServer;
