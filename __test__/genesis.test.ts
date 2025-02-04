import { http, createPublicClient } from "cive";
import { privateKeyToAccount } from "cive/accounts";
import { base32AddressToHex } from "cive/utils";
import { describe, expect, test } from "vitest";
import { createServer } from "../index";
import { TEST_NETWORK_ID, TEST_PRIVATE_KEYS, getFreePorts } from "./help";

/**
 * Test genesis configuration
 * Shows how to:
 * 1. Configure initial accounts in both Core space and EVM space
 * 2. Verify initial balances through both Core and EVM RPC endpoints
 */
describe("Genesis Configuration", () => {
  test("should initialize accounts with correct balances in both spaces", async () => {
    // Setup server with dual-space support
    const [jsonrpcHttpPort, jsonrpcHttpEthPort, udpAndTcpPort] =
      await getFreePorts();
    const server = await createServer({
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,
      chainId: TEST_NETWORK_ID,
      jsonrpcHttpPort: jsonrpcHttpPort,
      jsonrpcHttpEthPort: jsonrpcHttpEthPort,
      genesisSecrets: TEST_PRIVATE_KEYS,
      // 0x prefix is optional
      genesisEvmSecrets: TEST_PRIVATE_KEYS,
    });

    await server.start();

    // Create Core space client
    const client = createPublicClient({
      transport: http(`http://127.0.0.1:${jsonrpcHttpPort}`),
    });

    // Get test account
    const testAccount = privateKeyToAccount(`0x${TEST_PRIVATE_KEYS[0]}`, {
      networkId: TEST_NETWORK_ID,
    });

    // Check Core space balance
    const coreBalance = await client.getBalance({
      address: testAccount.address,
    });
    expect(coreBalance).toBe(10000000000000000000000n); // 10,000 CFX

    // Check EVM space balance
    const evmBalance = await fetch(`http://127.0.0.1:${jsonrpcHttpEthPort}`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        method: "eth_getBalance",
        params: [
          base32AddressToHex({ address: testAccount.address }),
          "latest",
        ],
        id: 1,
      }),
    }).then((res) => res.json());

    expect(BigInt(evmBalance.result)).toBe(10000000000000000000000n); // 10,000 CFX

    await server.stop();
  });
});
