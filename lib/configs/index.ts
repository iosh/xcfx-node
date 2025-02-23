import path from "path";
import { ConfluxConfig } from "../../conflux";
import { Config, DEFAULT_CONFIG } from "../types";

export const buildConfig = (config: Config): ConfluxConfig => {
  const finalConfig = { ...DEFAULT_CONFIG, ...config };

  return {
    posConfigPath: path.join(__dirname, "./pos_config/pos_config.yaml"),
    posInitialNodesPath: path.join(
      __dirname,
      "./pos_config/initial_nodes.json"
    ),
    logConf: finalConfig.log ? path.join(__dirname, "./log.yaml") : undefined,
    ...config,
  };
};
