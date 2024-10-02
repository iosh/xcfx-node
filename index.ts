import path from "node:path";
import { runNode, stopNode, type ConfluxConfig } from "./conflux";

export type { ConfluxConfig };

export async function createServer(userConfig: ConfluxConfig = {}) {
  const config: ConfluxConfig = {
    posConfigPath: path.join(__dirname, "./pos_config/pos_config.yaml"),
    posInitialNodesPath: path.join(
      __dirname,
      "./pos_config/initial_nodes.json",
    ),
    posPrivateKeyPath: path.join(__dirname, "./pos_config/pos_key"),
    ...userConfig,
  };
  return {
    async start() {
      runNode(config);
    },
    async stop() {
      stopNode();
    },
  };
}
