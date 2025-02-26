import { Config } from "./lib/types";
import { ConfluxInstance } from "./lib/conflux-instance";
export interface CreateServerReturnType {
  start: () => Promise<void>;
  stop: () => Promise<void>;
}

/**
 * Creates a new Conflux server instance
 * @param config - Server configuration options
 * @returns Object with start and stop methods
 */
export const createServer = async (
  config: Config = {},
): Promise<CreateServerReturnType> => {
  const instance = new ConfluxInstance(config);
  return {
    start: () => instance.start(),
    stop: () => instance.stop(),
  };
};
