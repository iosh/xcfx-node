import { http, createPublicClient, webSocket } from "cive";
import { privateKeyToAccount } from "cive/accounts";
import { parseCFX } from "cive/utils";
import { beforeAll, describe, expect, test } from "vitest";
import { createServer } from "../index";
import {
  MINING_ACCOUNT,
  TEST_MINING_ADDRESS,
  TEST_NETWORK_ID,
  TEST_PK,
  jsonrpcHttpPort,
  jsonrpcWsPort,
  localChain,
  udpAndTcpPort,
  wait,
} from "./help";

beforeAll(async () => {
  const server = await createServer({
    nodeType: "full",
    devBlockIntervalMs: 100,
    miningAuthor: TEST_MINING_ADDRESS,
    jsonrpcHttpPort: jsonrpcHttpPort,
    jsonrpcWsPort: jsonrpcWsPort,
    chainId: TEST_NETWORK_ID,
    evmChainId: 2222,
    genesisSecrets: TEST_PK,
    // devPackTxImmediately: true,
    tcpPort: udpAndTcpPort,
    udpPort: udpAndTcpPort,
  });

  await server.start();

  await wait(15000);

  return () => server.stop();
});

describe("customConfig", () => {
  test("test http port", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http(),
    });

    expect((await client.getStatus()).chainId).toBe(1111);
  });

  test("test ws port", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: webSocket(),
    });

    expect((await client.getStatus()).chainId).toBe(1111);
  });

  test("test mining address", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http(),
    });

    expect(
      await client.getBalance({
        address: MINING_ACCOUNT.address,
      }),
    ).toBeGreaterThan(0n);
  });

  test("auto mining", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http(),
    });
    const block = await client.getBlock();
    expect(block.blockNumber).toBeGreaterThan(0);
    expect(block.baseFeePerGas).toBeDefined();
  });

  test("test genesis", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http(),
    });

    for (const pk of TEST_PK) {
      const account = privateKeyToAccount(`0x${pk}`, {
        networkId: TEST_NETWORK_ID,
      });
      const balance = await client.getBalance({
        address: account.address,
      });
      expect(balance).toBe(parseCFX("10000"));
    }
  });
});
