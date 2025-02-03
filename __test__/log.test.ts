import { describe, expect, test, beforeAll, afterAll } from "vitest";
import { createServer } from "../index";
import { getFreePorts, sleep, TEST_TEMP_DATA_DIR, retryDelete } from "./help";
import fs from "node:fs";
import path from "node:path";

describe("test log", () => {
  const TEST_LOG_PATH = path.join(TEST_TEMP_DATA_DIR, "/log/log/test.log");
  const WORK_DIR = path.join(TEST_TEMP_DATA_DIR, "/log");

  afterAll(async () => {
    if (fs.existsSync(TEST_LOG_PATH)) {
      try {
        await retryDelete(WORK_DIR, true);
      } catch (error) {
        console.warn(
          `Warning: Failed to clean up test files: ${error.message}`
        );
      }
    }
  });

  test("test custom log", async () => {
    const [jsonrpcHttpPort, udpAndTcpPort] = await getFreePorts();

    const server = await createServer({
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,
      jsonrpcHttpPort: jsonrpcHttpPort,
      logConf: path.join(__dirname, "fixtures/custom_log.yaml"),
      dataDir: WORK_DIR,
    });

    // start server
    await server.start();

    // wait for log generate
    await new Promise((resolve) => setTimeout(resolve, 2000));

    // check log file is created
    expect(fs.existsSync(TEST_LOG_PATH)).toBe(true);

    // read log content
    const logContent = fs.readFileSync(TEST_LOG_PATH, "utf-8");

    // check log content
    expect(logContent).toContain("DEBUG");
    // stop server
    await server.stop();
  });
});
