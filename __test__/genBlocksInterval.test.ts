import { http, createPublicClient } from "cive";
import { beforeAll, describe, expect, test } from "vitest";
import { createServer } from "../index";
import { getFreePorts, localChain, wait } from "./help";

/**
 * Test automatic block generation functionality
 * Shows how to:
 * 1. Configure automatic block generation interval
 * 2. Verify blocks are generated at the specified interval
 * 3. Monitor epoch number progression
 */
describe("Automatic Block Generation", () => {
  let httpPort: number;

  // Setup a node with automatic block generation
  beforeAll(async () => {
    const [jsonrpcHttpPort, udpAndTcpPort] = await getFreePorts();
    httpPort = jsonrpcHttpPort;
    
    const server = await createServer({
      // Configure block generation every 100ms
      devBlockIntervalMs: 100,
      
      // Node endpoints
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,
      jsonrpcHttpPort,
    });

    await server.start();
    return () => server.stop();
  });

  test("should generate blocks at configured interval", async () => {
    // Wait for initial block generation
    await wait(2000);
    
    const client = createPublicClient({
      chain: localChain,
      transport: http(`http://127.0.0.1:${httpPort}`),
    });

    // Check first epoch number
    await wait(1000);
    const status1 = await client.getStatus();
    expect(status1.epochNumber).toBeGreaterThan(1);

    // Verify epoch number increases
    await wait(1000);
    const status2 = await client.getStatus();
    expect(status2.epochNumber).toBeGreaterThan(status1.epochNumber);

    // Verify continuous block generation
    await wait(1000);
    const status3 = await client.getStatus();
    expect(status3.epochNumber).toBeGreaterThan(status2.epochNumber);
  });
});
