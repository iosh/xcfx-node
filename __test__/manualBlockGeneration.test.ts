import {
  http,
  createPublicClient,
  createTestClient,
  createWalletClient,
} from "cive";
import { privateKeyToAccount } from "cive/accounts";
import { parseCFX } from "cive/utils";
import { beforeAll, describe, expect, test } from "vitest";
import { createServer } from "../index";
import {
  TEST_NETWORK_ID,
  TEST_PRIVATE_KEYS,
  getFreePorts,
  localChain,
  wait,
} from "./help";

/**
 * Test manual block generation functionality
 * Shows how to:
 * 1. Configure a node with manual block generation
 * 2. Verify block generation behavior
 * 3. Test transaction handling without auto-mining
 * 4. Manually generate blocks using test client
 */
describe("Manual Block Generation", () => {
  const testAccount = privateKeyToAccount(`0x${TEST_PRIVATE_KEYS[0]}`, {
    networkId: TEST_NETWORK_ID,
  });
  let httpPort: number;

  // Setup a node with manual block generation
  beforeAll(async () => {
    const [jsonrpcHttpPort, udpAndTcpPort] = await getFreePorts();
    httpPort = jsonrpcHttpPort;

    const server = await createServer({
      // Node configuration
      nodeType: "full",
      jsonrpcHttpPort,
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,

      // Chain configuration
      chainId: TEST_NETWORK_ID,
      evmChainId: 2222,
      genesisSecrets: TEST_PRIVATE_KEYS,

      // Disable automatic block packaging
      devPackTxImmediately: false,
    });

    await server.start();

    return () => server.stop();
  });

  test("should not generate blocks automatically", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http(`http://127.0.0.1:${httpPort}`),
    });

    // Check block number at different intervals
    const status1 = await client.getStatus();
    await wait(1000);

    const status2 = await client.getStatus();
    expect(status1.blockNumber).toBe(status2.blockNumber);
    await wait(1000);

    const status3 = await client.getStatus();
    expect(status2.blockNumber).toBe(status3.blockNumber);
    await wait(1000);

    const status4 = await client.getStatus();
    expect(status3.blockNumber).toBe(status4.blockNumber);
  });

  test("should not generate blocks when receiving transactions", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http(`http://127.0.0.1:${httpPort}`),
    });

    const walletClient = createWalletClient({
      account: testAccount,
      chain: localChain,
      transport: http(`http://127.0.0.1:${httpPort}`),
    });

    // Record initial block number
    const status1 = await client.getStatus();

    // Send a transaction
    await walletClient.sendTransaction({
      to: testAccount.address,
      value: parseCFX("1"),
    });

    // Verify block number hasn't changed
    await wait(1000);
    const status2 = await client.getStatus();
    expect(status1.blockNumber).toBe(status2.blockNumber);
  });

  test("should generate blocks manually using test client", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http(`http://127.0.0.1:${httpPort}`),
    });

    const testClient = createTestClient({
      chain: localChain,
      transport: http(`http://127.0.0.1:${httpPort}`),
    });

    // Record initial block number
    const status1 = await client.getStatus();

    // Generate 10 blocks manually
    await testClient.mine({ blocks: 10 });

    // Verify block number increased by 10
    const status2 = await client.getStatus();
    expect(status2.blockNumber).toBe(status1.blockNumber + 10n);
  });
});
