import { createPublicClient, http } from "cive";
import { describe, expect, test } from "vitest";
import { createServer } from "../index";
import { getFreePorts } from "./help";

/**
 * Test basic server functionality
 * Shows how to:
 * 1. Start and stop a node with minimal configuration
 * 2. Connect to the node via HTTP RPC
 * 3. Verify basic node information
 * 4. Handle server shutdown
 */
describe("Server Lifecycle", () => {
  test("should handle basic server operations", async () => {
    // Start server with minimal configuration
    const [jsonrpcHttpPort, udpAndTcpPort] = await getFreePorts();
    const server = await createServer({
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,
      jsonrpcHttpPort: jsonrpcHttpPort,
    });
    await server.start();

    // Create RPC client
    const client = createPublicClient({
      transport: http(`http://127.0.0.1:${jsonrpcHttpPort}`),
    });

    // Verify node status
    const status = await client.getStatus();
    expect(status.chainId).toBe(1234); // Default chain ID
    expect(status.networkId).toBe(1234); // Default network ID

    // Check node version
    const version = await client.getClientVersion();
    expect(version).toContain("conflux-rust/v3.0.2");

    // Test server shutdown
    await server.stop();

    // Verify server is no longer accessible
    await expect(async () => client.getStatus()).rejects.toThrow();
  });
});
