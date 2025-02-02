import { describe, expect, test, beforeAll, afterAll, afterEach } from "vitest";
import { createServer } from "../index";
import { getFreePorts, sleep, TEST_TEMP_DATA_DIR } from "./help";
import fs from "node:fs";
import path from "node:path";

describe("Data Directory Tests", () => {
  const TEST_DATA_DIR = path.join(TEST_TEMP_DATA_DIR, "dataDir");

  // Clean test directory and get ports before all tests
  beforeAll(async () => {
    if (fs.existsSync(TEST_DATA_DIR)) {
      fs.rmSync(TEST_DATA_DIR, { recursive: true, force: true });
    }
  });

  // Clean up after all tests
  afterAll(async () => {
    if (fs.existsSync(TEST_DATA_DIR)) {
      await sleep(1000);
      fs.rmSync(TEST_DATA_DIR, { recursive: true, force: true });
    }
  });

  test("custom data directory", async () => {
    const ports = await getFreePorts();
    const server = await createServer({
      tcpPort: ports[1],
      udpPort: ports[1],
      jsonrpcHttpPort: ports[0],
      dataDir: TEST_DATA_DIR,
    });

    // Start server
    await server.start();

    expect(fs.existsSync(TEST_DATA_DIR)).toBe(true);

    const files = fs.readdirSync(TEST_DATA_DIR);
    expect(files.length).toBeGreaterThan(0);
  });
});
