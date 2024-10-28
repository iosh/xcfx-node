import { http, createPublicClient, webSocket, createWalletClient } from "cive";
import { privateKeyToAccount } from "cive/accounts";
import { parseCFX } from "cive/utils";
import { beforeAll, describe, expect, test } from "vitest";
import { createServer } from "../index";
import {
  TEST_NETWORK_ID,
  TEST_PK,
  getFreePorts,
  localChain,
  wait,
} from "./help";

const account = privateKeyToAccount(`0x${TEST_PK[0]}`, {
  networkId: TEST_NETWORK_ID,
});
let HTTP_PORT: number;
beforeAll(async () => {
  const [jsonrpcHttpPort, udpAndTcpPort] = await getFreePorts();
  HTTP_PORT = jsonrpcHttpPort;
  const server = await createServer({
    nodeType: "full",
    jsonrpcHttpPort: jsonrpcHttpPort,
    chainId: TEST_NETWORK_ID,
    evmChainId: 2222,
    devPackTxImmediately: false,
    genesisSecrets: TEST_PK,
    tcpPort: udpAndTcpPort,
    udpPort: udpAndTcpPort,
  });

  await server.start();

  await wait(10000);
  return () => server.stop();
});

describe("manual block generation", async () => {
  test("block not changed", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http(`http://127.0.0.1:${HTTP_PORT}`),
    });

    await wait(1000);
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

  test("block not change in receive tx", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http(`http://127.0.0.1:${HTTP_PORT}`),
    });

    const walletClient = createWalletClient({
      account,
      chain: localChain,
      transport: http(`http://127.0.0.1:${HTTP_PORT}`),
    });
    const status = await client.getStatus();

    await walletClient.sendTransaction({
      to: account.address,
      value: parseCFX("1"),
    });

    await wait(1000);
    const status2 = await client.getStatus();
    expect(status.blockNumber).toBe(status2.blockNumber);
  });

  test("gen block", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http(`http://127.0.0.1:${HTTP_PORT}`),
    });

    const status = await client.getStatus();

    await client.request<any>({
      method: "generate_empty_blocks",
      params: [10],
    });

    const status1 = await client.getStatus();
    expect(status1.blockNumber).toBe(status.blockNumber + 10n);
  });
});
