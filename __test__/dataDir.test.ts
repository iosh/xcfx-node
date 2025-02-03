import { describe, expect, test, afterEach } from "vitest";
import { createServer } from "../index";
import { getFreePorts, retryDelete, TEST_TEMP_DATA_DIR } from "./help";
import fs from "node:fs";
import path from "node:path";

/**
 * Test data directory configuration
 * Shows how to set up a custom data directory for the node
 */
describe("Data Directory", () => {
  const TEST_DATA_DIR = path.join(TEST_TEMP_DATA_DIR, "dataDir");

  // Cleanup after each test
  afterEach(async () => {
    if (fs.existsSync(TEST_DATA_DIR)) {
      await retryDelete(TEST_DATA_DIR, true);
    }
  });

  test("should create and use custom data directory", async () => {
    const ports = await getFreePorts();
    const server = await createServer({
      tcpPort: ports[0],
      udpPort: ports[0],
      jsonrpcHttpPort: ports[1],
      dataDir: TEST_DATA_DIR,
    });

    await server.start();
    
    // Verify data directory exists
    expect(fs.existsSync(TEST_DATA_DIR)).toBe(true);
    
    await server.stop();
  });
});
