import { http, createPublicClient } from "cive";
import { beforeAll, describe, expect, test } from "vitest";
import { createServer } from "../index";
import { getFreePorts, localChain, wait } from "./help";

let HTTP_PORT: number;

beforeAll(async () => {
  // blocks are automatically generated every ``dev_block_interval_ms'' ms.
  const [jsonrpcHttpPort, udpAndTcpPort] = await getFreePorts();
  HTTP_PORT = jsonrpcHttpPort;
  const server = await createServer({
    devBlockIntervalMs: 100,
    tcpPort: udpAndTcpPort,
    udpPort: udpAndTcpPort,
    jsonrpcHttpPort: jsonrpcHttpPort,
  });

  await server.start();
  return () => server.stop();
});

describe("genBlocksInterval", () => {
  test("default", async () => {
    const client = createPublicClient({
      chain: localChain,
      transport: http(`http://127.0.0.1:${HTTP_PORT}`),
    });

    await wait(200);
    const status1 = await client.getStatus();
    expect(status1.epochNumber).toBeGreaterThan(1);

    await wait(200);
    const status2 = await client.getStatus();
    expect(status2.epochNumber).toBeGreaterThan(status1.epochNumber);

    await wait(200);
    const status3 = await client.getStatus();
    expect(status3.epochNumber).toBeGreaterThan(status2.epochNumber);

    await wait(200);
    const status4 = await client.getStatus();
    expect(status4.epochNumber).toBeGreaterThan(status3.epochNumber);
  });
});
