import { describe, expect, test, afterEach } from "vitest";
import { createServer } from "../index";
import {
  getFreePorts,
  localChain,
  retryDelete,
  sleep,
  TEST_NETWORK_ID,
  TEST_PRIVATE_KEYS,
  TEST_TEMP_DATA_DIR,
} from "./help";
import fs from "node:fs";
import path from "node:path";
import {
  createPublicClient,
  createTestClient,
  createWalletClient,
  http,
  parseCFX,
} from "cive";
import { privateKeyToAccount } from "cive/accounts";

/**
 * Test data directory configuration
 * Shows how to set up a custom data directory for the node
 */
describe("Data Directory", () => {
  const TEST_DATA_DIR = path.join(TEST_TEMP_DATA_DIR, "dataDir");

  // Cleanup after each test
  afterEach(async () => {
    if (fs.existsSync(TEST_DATA_DIR)) {
      await retryDelete(TEST_DATA_DIR, true);
    }
  });

  test("should create and use custom data directory", async () => {
    const [jsonrpcHttpPort, udpAndTcpPort] = await getFreePorts();
    const server = await createServer({
      // tcpPort: udpAndTcpPort,
      // udpPort: udpAndTcpPort,
      jsonrpcHttpPort: jsonrpcHttpPort,
      genesisSecrets: TEST_PRIVATE_KEYS,
      confluxDataDir: TEST_DATA_DIR,
      chainId: TEST_NETWORK_ID,

      posReferenceEnableHeight: 0,
      devPackTxImmediately: true,
      // devBlockIntervalMs: 1000,
      // log: true,
    });

    await server.start();

    // Verify data directory exists
    expect(fs.existsSync(TEST_DATA_DIR)).toBe(true);

    const account = privateKeyToAccount(`0x${TEST_PRIVATE_KEYS[0]}`, {
      networkId: TEST_NETWORK_ID,
    });
    const walletClient = createWalletClient({
      chain: localChain,
      account,
      transport: http(`http://127.0.0.1:${jsonrpcHttpPort}`),
    });
    const client = createPublicClient({
      chain: localChain,
      transport: http(`http://127.0.0.1:${jsonrpcHttpPort}`),
    });

    const testClient = createTestClient({
      transport: http(`http://127.0.0.1:${jsonrpcHttpPort}`),
    });

    await testClient.mine({ blocks: 10 });

    const epochNumber = await client.getEpochNumber();

    await walletClient.sendTransaction({
      to: account.address,
      value: parseCFX("1"),
    });

    const balance = await client.getBalance({ address: account.address });

    await server.stop();

    const server2 = await createServer({
      jsonrpcHttpPort: jsonrpcHttpPort,
      genesisSecrets: TEST_PRIVATE_KEYS,
      confluxDataDir: TEST_DATA_DIR,
      chainId: TEST_NETWORK_ID,

      posReferenceEnableHeight: 0,
      devPackTxImmediately: true,
    });


    await server2.start();
    const epochNumber2 = await client.getEpochNumber();

    expect(Number(epochNumber2)).greaterThanOrEqual(Number(epochNumber));

    const balance2 = await client.getBalance({ address: account.address });

    expect(balance2).toEqual(balance);

    await server2.stop();
  });
});
