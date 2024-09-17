import path from "node:path";
import { createConfigFile } from "./createConfigFile";
import type { ConfluxConfig, ServerConfig } from "./types";
import {
  getBinPath,
  cleanup,
  checkPort,
  checkIsConfluxNodeRunning,
} from "./helper";
import { ConfluxServer } from "./server";

export type createServerReturnType = {
  start: () => Promise<void>;
  stop: () => Promise<void>;
};

async function createServer(
  config: ConfluxConfig & ServerConfig = {}
): Promise<createServerReturnType> {
  const configWithDefault = {
    ...config,
    mode: config.mode || "dev",
    waitUntilReady: config.waitUntilReady || true,
    silent: config.silent || true,
    persistNodeData: config.persistNodeData || false,

    // default port
    jsonrpc_http_port: config.jsonrpc_http_port || 12537,
    jsonrpc_ws_port: config.jsonrpc_ws_port || 12535,
    chain_id: config.chain_id || 1234,
    evm_chain_id: config.evm_chain_id || 1235,
  };

  if (await checkPort(configWithDefault.jsonrpc_http_port)) {
    // check it is conflux node
    if (
      await checkIsConfluxNodeRunning({
        chainId: configWithDefault.chain_id,
        httpPort: configWithDefault.jsonrpc_http_port,
      })
    ) {
      throw new Error(
        `The Conflux node is already running on the port ${configWithDefault.jsonrpc_http_port}.`
      );
    }

    throw new Error(
      `The jsonrpc_http_port port ${configWithDefault.jsonrpc_http_port} is already in use.`
    );
  }

  if (await checkPort(configWithDefault.jsonrpc_ws_port)) {
    if (
      await checkIsConfluxNodeRunning({
        chainId: configWithDefault.chain_id,
        wsPort: configWithDefault.jsonrpc_ws_port,
      })
    ) {
      throw new Error(
        `The Conflux node is already running on the port ${configWithDefault.jsonrpc_ws_port}.`
      );
    }

    throw new Error(
      `The jsonrpc_ws_port port ${configWithDefault.jsonrpc_ws_port} is already in use.`
    );
  }

  const BinPathResult = getBinPath();

  if ("errorMessage" in BinPathResult) {
    throw new Error(BinPathResult.errorMessage);
  }
  const workDir = path.join(__dirname, "../data");
  // try to cleanup the data dir
  if (!configWithDefault.persistNodeData) {
    cleanup(workDir);
  }

  await createConfigFile(configWithDefault);

  const server = new ConfluxServer(configWithDefault);

  return server;
}

export default createServer;
