import fs from "node:fs";
import path from "node:path";
import detectPort from "detect-port";
import {
  Chain,
  createPublicClient,
  http,
  PublicClient,
  Transport,
  webSocket,
} from "cive";
import { defineChain } from "cive/utils";

type CheckEnvironmentReturnType =
  | {
      binPath: string;
    }
  | {
      errorMessage: string;
    };

/**
 * Check the current platform and architecture is supported
 * @returns { CheckEnvironmentReturnType }
 */
export function getBinPath(): CheckEnvironmentReturnType {
  const { platform, arch } = process;

  // Currently only limited platforms are supported.
  const ALL_SUPPORT_PLATFORMS: Partial<
    Record<typeof platform, Partial<Record<typeof arch, string>>>
  > = {
    linux: {
      x64: "@xcfx/node-linux-x64",
    },
    darwin: {
      arm64: "@xcfx/node-darwin-arm64",
    },
  };

  if (!ALL_SUPPORT_PLATFORMS[platform]) {
    return {
      errorMessage: `@xcfx/node is not supported the current platform: ${platform} yet. You can submit an issue: https://github.com/iosh/xcfx-node/issues/new`,
    };
  }

  if (!ALL_SUPPORT_PLATFORMS[platform][arch]) {
    return {
      errorMessage: `@xcfx/node is not supported the current architecture: ${arch} yet. You can submit an issue: https://github.com/iosh/xcfx-node/issues/new`,
    };
  }

  const BIN_NAME = `${ALL_SUPPORT_PLATFORMS[platform][arch]}/conflux`;

  return {
    binPath: require.resolve(BIN_NAME),
  };
}

const rmsyncOptions = {
  force: true,
  recursive: true,
};

export function cleanup(workDir: string) {
  // remove conflux data
  fs.rmSync(path.join(workDir, "blockchain_data"), rmsyncOptions);
  // remove conflux db
  fs.rmSync(path.join(workDir, "db"), rmsyncOptions);
  // remove conflux log
  fs.rmSync(path.join(workDir, "log"), rmsyncOptions);
  // remove conflux pos_db
  fs.rmSync(path.join(workDir, "pos_db"), rmsyncOptions);
}

/**
 * Check the port is in used or not
 * @param port the port to check
 * @returns
 */
export async function checkPort(port: number) {
  const p = await detectPort(port);
  if (p === port) {
    return false;
  }

  return true;
}

type createChainParamsType = {
  chainId: number;
  url?: string | undefined;
  wsUrl?: string | undefined;
};

function createChain({ chainId, url, wsUrl }: createChainParamsType) {
  return defineChain({
    id: chainId,
    name: "Localhost",
    nativeCurrency: {
      decimals: 18,
      name: "CFX",
      symbol: "CFX",
    },
    rpcUrls: {
      default: {
        http: url ? [url] : [],
        webSocket: wsUrl ? [wsUrl] : [],
      },
    },
  });
}

export type checkIsConfluxNodeRunningParamsType = {
  chainId: number;
} & (
  | {
      wsPort?: undefined;
      httpPort: number;
    }
  | {
      httpPort?: undefined;
      wsPort: number;
    }
);

export async function checkIsConfluxNodeRunning(
  args: checkIsConfluxNodeRunningParamsType
) {
  const chain = createChain({
    chainId: args.chainId,
    url: args.httpPort ? `http://127.0.0.1:${args.httpPort}` : undefined,
    wsUrl: args.wsPort ? `ws://127.0.0.1:${args.wsPort}` : undefined,
  });
  let client: PublicClient<Transport, Chain>;

  if (args.httpPort) {
    client = createPublicClient({
      chain,
      transport: http(),
    });
  } else {
    client = createPublicClient({
      chain,
      transport: webSocket(),
    });
  }

  try {
    const data = await client.getClientVersion();
    if (data) {
      return true;
    }
    return false;
  } catch (error) {
    return false;
  }
}

type WaitConfluxNodeReadyParamsType = {
  chainId: number;
  httpPort: number;
};

export async function waitConfluxNodeReady({
  chainId,
  httpPort,
}: WaitConfluxNodeReadyParamsType) {
  const chain = createChain({
    chainId,
    url: `http://127.0.0.1:${httpPort}`,
  });
  const client = createPublicClient({
    chain,
    transport: http(),
  });
 
  await client.request(
    {
      method: "cfx_getStatus",
    },
    {
      retryCount: 50,
      retryDelay: 500,
    }
  );
}
