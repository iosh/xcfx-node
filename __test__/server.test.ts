import { http, createPublicClient } from "cive";
import { beforeAll, describe, expect, test } from "vitest";
import { createServer } from "../index";
import { localChain } from "./help";

describe("server", () => {
  test("default", async () => {
    const server = await createServer();
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
    await new Promise((resolve) => setTimeout(resolve, 3000));
  });
});
