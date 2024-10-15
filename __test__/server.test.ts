import { http, createPublicClient } from "cive";
import { describe, expect, test } from "vitest";
import { createServer } from "../index";
import { jsonrpcHttpPort, localChain, udpAndTcpPort } from "./help";

describe("server", () => {
  test("default", async () => {
    const server = await createServer({
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,
      jsonrpcHttpPort: jsonrpcHttpPort,
    });
    await server.start();
    // TODO update this
    await new Promise((resolve) => setTimeout(resolve, 15000));
    const client = createPublicClient({
      chain: localChain,
      transport: http(),
    });

    const status = await client.getStatus();

    expect(status.chainId).toBe(1234);
    expect(status.networkId).toBe(1234);

    await server.stop();

    await expect(async () => client.getStatus()).rejects.toThrow();
  });
});
