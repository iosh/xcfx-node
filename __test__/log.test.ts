import { describe, expect, test, afterAll } from "vitest";
import { createServer } from "../index";
import { getFreePorts, TEST_TEMP_DATA_DIR, retryDelete } from "./help";
import fs from "node:fs";
import path from "node:path";

/**
 * Test log configuration
 * Shows how to set up custom logging with a YAML config file
 */
describe("Logging", () => {
  const TEST_LOG_PATH = path.join(TEST_TEMP_DATA_DIR, "/log/log/test.log");
  const WORK_DIR = path.join(TEST_TEMP_DATA_DIR, "/log");

  afterAll(async () => {
    if (fs.existsSync(WORK_DIR)) {
      await retryDelete(WORK_DIR);
    }
  });

  test("should use custom log configuration file", async () => {
    const ports = await getFreePorts();
    const server = await createServer({
      tcpPort: ports[0],
      udpPort: ports[0],
      jsonrpcHttpPort: ports[1],
      logConf: path.join(__dirname, "fixtures/custom_log.yaml"),
      confluxDataDir: WORK_DIR,
    });

    await server.start();

    // Wait for log4rs to initialize and write logs
    await new Promise((resolve) => setTimeout(resolve, 2000));

    // Verify log file exists and contains log entries
    expect(fs.existsSync(TEST_LOG_PATH)).toBe(true);
    const logContent = fs.readFileSync(TEST_LOG_PATH, "utf-8");
    // Log format should match pattern from log4rs config
    expect(logContent).toMatch(/\d{4}-\d{2}-\d{2}/); // Date format

    await server.stop();
  });

  // test("should use default console logging", async () => {
  //   const ports = await getFreePorts();
  //   const server = await createServer({
  //     tcpPort: ports[0],
  //     udpPort: ports[0],
  //     jsonrpcHttpPort: ports[1],
  //     confluxDataDir: WORK_DIR,
  //     log: true, // This will use the default log.yaml in configs the log will print to console
  //   });

  //   await server.start();
  //   await server.stop();
  // });
});
