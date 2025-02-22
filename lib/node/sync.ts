import { createTestClient, http, webSocket } from "cive";
import { NodeRequestOptions } from "../types";

export const waitForNodeRPCReady = async (config: NodeRequestOptions) => {
  // if no ports are provided, return because there is no rpc to setup
  if (!config.httpPort && !config.wsPort) return;

  const transport = config.httpPort
    ? http(`http://127.0.0.1:${config.httpPort}`)
    : webSocket(`ws://127.0.0.1:${config.wsPort}`);

  const testClient = createTestClient({ transport });

  const abortController = new AbortController();

  const timeoutId = setTimeout(() => {
    abortController.abort();
  }, config.timeout);

  try {
    while (!abortController?.signal.aborted) {
      const phase = await testClient.getCurrentSyncPhase();
      if (phase === "NormalSyncPhase") {
        clearTimeout(timeoutId);
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, config.retryInterval));
    }
  } catch (error) {
    if (abortController?.signal.aborted) {
      throw new Error("Get node sync phase timeout");
    }
    throw error;
  } finally {
    clearTimeout(timeoutId);
  }
};
