import { http, createPublicClient, webSocket } from "cive";
import { privateKeyToAccount } from "cive/accounts";
import { parseCFX } from "cive/utils";
import { beforeAll, describe, expect, test } from "vitest";
import { createServer } from "../index";
import { getPortFree, localChain } from "./help";

const TEST_MINING_ADDRESS = "0x1b13CC31fC4Ceca3b72e3cc6048E7fabaefB3AC3";
const TEST_MINING_ADDRESS_PK =
  "0xe590669d866ccd3baf9ec816e3b9f50a964b678e8752386c0d81c39d7e9c6930";
const TEST_NETWORK_ID = 1111;

const miningAccount = privateKeyToAccount(TEST_MINING_ADDRESS_PK, {
  networkId: TEST_NETWORK_ID,
});

const TEST_PK = [
  "5880f9c6c419f4369b53327bae24e62d805ab0ff825acf7e7945d1f9a0a17bd0",
  "346328c0b7e2abf88fd9c5126192a85eb18b5309c3bd3cc7b61c8b403d1b0e68",
  "bdc6d1c95aaaac8db4eaa5d3d097ba556cf2871149f31e695458d7813543c258",
  "8d108e2aff19b1134c7cc29fbe4a721e599ef21d7d183f64dcd5e78e3a12e5e7",
  "e9cfd4d1d29f7a67c970c1d5c145e958061cd54d05a83e29c7e39b7be894c9c6",
];

beforeAll(async () => {
  const tcpAndUdpPort = await getPortFree();
  const server = await createServer({
    nodeType: "full",
    devBlockIntervalMs: 200,
    miningAuthor: TEST_MINING_ADDRESS,
    jsonrpcHttpPort: 12555,
    jsonrpcWsPort: 12556,
    chainId: TEST_NETWORK_ID,
    evmChainId: 2222,
    genesisSecrets: TEST_PK,
    // devPackTxImmediately: true,
    tcpPort: tcpAndUdpPort,
    udpPort: tcpAndUdpPort,
  });

  await server.start();

  await new Promise((resolve) => setTimeout(resolve, 15000));

  return () => server.stop();
});

describe("customConfig", () => {
  test("test http port", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http("http://127.0.0.1:12555"),
    });

    expect((await client.getStatus()).chainId).toBe(1111);
  });

  test("test ws port", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: webSocket("ws://127.0.0.1:12556"),
    });

    expect((await client.getStatus()).chainId).toBe(1111);
  });

  test("test mining address", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http("http://127.0.0.1:12555"),
    });

    expect(
      await client.getBalance({
        address: miningAccount.address,
      }),
    ).toBeGreaterThan(0n);
  });

  test("auto mining", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http("http://127.0.0.1:12555"),
    });
    const block = await client.getBlock();
    expect(block.blockNumber).toBeGreaterThan(0);
    expect(block.baseFeePerGas).toBeDefined();
  });

  test("test genesis", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http("http://127.0.0.1:12555"),
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
