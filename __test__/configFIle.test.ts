import { describe, expect, test } from "vitest";
import { createServer } from "../index";
import { join } from "path";
import { createPublicClient, http } from "cive";

describe("configFile", () => {
  test("should load config file", async () => {
    const server = await createServer({
      configFile: join(__dirname, "./fixtures/testConfig/config.toml"),
    });

    await server.start();

    const client = createPublicClient({
      transport: http("http://localhost:12537"),
    });

    const status = await client.getStatus();

    expect(status.networkId).toBe(1);
    expect(status.chainId).toBe(1);
    expect(status.ethereumSpaceChainId).toBe(71);

    await server.stop();
  });
});
