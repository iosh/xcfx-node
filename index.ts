import path from "node:path";
import { type ConfluxConfig, ConfluxNode } from "./conflux";

export type Config = {
  /**
   * show conflux node log
   * @default false
   */
  log?:boolean
} & ConfluxConfig;

let isServiceCreated = false;
export async function createServer(config: Config = {}) {
  if (isServiceCreated) {
    throw new Error("The server has already been created");
  }
  isServiceCreated = true;

  const {log = false, ...userConfig} = config

  const filledConfig: ConfluxConfig = {
    posConfigPath: path.join(__dirname, "./configs/pos_config/pos_config.yaml"),
    posInitialNodesPath: path.join(
      __dirname,
      "./configs/pos_config/initial_nodes.json",
    ),
    posPrivateKeyPath: path.join(__dirname, "../configs/pos_config/pos_key"),

    logConf: log ? path.join(__dirname, "./configs/log4rs.yaml") : undefined,
    
    ...userConfig,
  };

  const node = new ConfluxNode();
  return {
    async start() {
      node.startNode(filledConfig);
    },
    async stop() {
      node.stopNode();
    },
  };
}
