import { describe, test, expect, beforeAll } from "vitest";
import { createServer } from "../index";
import { createPublicClient, http } from "cive";
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
    expect(status.networkId).toBe(1235);
  });
});
