import { http, createPublicClient } from "cive";
import { describe, expect, test } from "vitest";
import { createServer } from "../index";
import { getFreePorts, localChain, wait } from "./help";

describe("server", () => {
  test("default", async () => {
    const [jsonrpcHttpPort, udpAndTcpPort] = await getFreePorts();
    const server = await createServer({
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,
      jsonrpcHttpPort: jsonrpcHttpPort,
    });
    await server.start();
    const client = createPublicClient({
      chain: localChain,
      transport: http(`http://127.0.0.1:${jsonrpcHttpPort}`),
    });

    const status = await client.getStatus();

    expect(status.chainId).toBe(1234);
    expect(status.networkId).toBe(1234);

    await server.stop();
    await expect(async () => client.getStatus()).rejects.toThrow();
  });
});
