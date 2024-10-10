import path from "node:path";
import { type ConfluxConfig, ConfluxNode } from "./conflux";

export type { ConfluxConfig };


let isServiceCreated = false;
export async function createServer(userConfig: ConfluxConfig = {}) {

  if (isServiceCreated) {
    throw new Error("The server has already been created");
  }
  isServiceCreated = true;

  const config: ConfluxConfig = {
    posConfigPath: path.join(__dirname, "./configs/pos_config/pos_config.yaml"),
    posInitialNodesPath: path.join(
      __dirname,
      "../configs/pos_config/initial_nodes.json",
    ),
    posPrivateKeyPath: path.join(__dirname, "../configs/pos_config/pos_key"),
    logConf: path.join(__dirname, "./configs/log.yaml"),
    ...userConfig,
  };

  const node = new ConfluxNode()
  return {
    async start() {
      node.startNode(config);
    },
    async stop() {
      node.stopNode();
    },
  };
}
