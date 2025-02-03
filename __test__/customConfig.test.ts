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
  getFreePorts,
  wait,
} from "./help";

/**
 * Test custom node configurations
 * Shows how to:
 * 1. Configure node type and mining settings
 * 2. Set up HTTP and WebSocket endpoints
 * 3. Configure chain IDs and genesis accounts
 * 4. Enable and test filter RPC
 */
describe("Custom Node Configuration", () => {
  let httpPort: number;
  let wsPort: number;

  // Set up a shared test node with custom configurations
  beforeAll(async () => {
    const [jsonrpcHttpPort, jsonrpcWsPort, udpAndTcpPort] =
      await getFreePorts();
    httpPort = jsonrpcHttpPort;
    wsPort = jsonrpcWsPort;

    const server = await createServer({
      // Node type and mining configuration
      nodeType: "full",
      devBlockIntervalMs: 100,
      miningAuthor: TEST_MINING_ADDRESS,

      // RPC endpoints
      jsonrpcHttpPort,
      jsonrpcWsPort,
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,

      // Chain configuration
      chainId: TEST_NETWORK_ID,
      evmChainId: 2222,
      genesisSecrets: TEST_PK,

      // Filter RPC configuration
      pollLifetimeInSeconds: 180,
    });

    await server.start();

    // wait for node to generate blocks

    await wait(7000);

    return () => server.stop();
  });

  test("should expose HTTP RPC endpoint", async () => {
    const client = createPublicClient({
      transport: http(`http://127.0.0.1:${httpPort}`),
    });

    const status = await client.getStatus();
    expect(status.chainId).toBe(1111);
  });

  test("should expose WebSocket RPC endpoint", async () => {
    const client = createPublicClient({
      transport: webSocket(`ws://127.0.0.1:${wsPort}`),
    });

    const status = await client.getStatus();
    expect(status.chainId).toBe(1111);
  });

  test("should mine blocks to configured mining address", async () => {
    const client = createPublicClient({
      transport: http(`http://127.0.0.1:${httpPort}`),
    });

    const balance = await client.getBalance({
      address: MINING_ACCOUNT.address,
    });
    expect(balance).toBeGreaterThan(0n);
  });

  test("should automatically generate blocks", async () => {
    const client = createPublicClient({
      transport: http(`http://127.0.0.1:${httpPort}`),
    });

    const block = await client.getBlock();
    expect(block.blockNumber).toBeGreaterThan(0);
    expect(block.baseFeePerGas).toBeDefined();
  });

  test("should initialize genesis accounts with correct balances", async () => {
    const client = createPublicClient({
      transport: http(`http://127.0.0.1:${httpPort}`),
    });

    for (const pk of TEST_PK) {
      const account = privateKeyToAccount(`0x${pk}`, {
        networkId: TEST_NETWORK_ID,
      });
      const balance = await client.getBalance({
        address: account.address,
      });
      expect(balance).toBe(parseCFX("10000")); // 10,000 CFX
    }
  });

  test("should enable filter RPC with configured lifetime", async () => {
    const client = createPublicClient({
      transport: http(`http://127.0.0.1:${httpPort}`),
    });

    const filterId = await client.createBlockFilter();
    expect(filterId).toBeDefined();
  });
});
