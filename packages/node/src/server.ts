import { type ChildProcessWithoutNullStreams, spawn } from "node:child_process";
import path from "node:path";
import checkEnvironment from "./checkEnvironment";
import { CONFIG_FILE_NAME, createConfigFile } from "./createConfigFile";
import type { ConfluxConfig, ServerConfig } from "./types";

import { cleanup } from "./cleanup";

async function createServer({
  waitUntilReady = true,
  silent = true,
  persistNodeData = false,
  ...config
}: ConfluxConfig & ServerConfig = {}) {
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
  return {
    start: async () => {
      return new Promise<void>((resolve) => {
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
    stop: async () => {
      if (conflux) {
        conflux.stdin.end();
        conflux.stdout.destroy();
        conflux.stdin.destroy();
        conflux.kill("SIGINT");

        if (!persistNodeData) {
          cleanup(workDir);
        }
      }
    },
  };
}

export default createServer;
