import { describe, test, expect } from "vitest";
import { createServer } from "../index";

describe("createServer", () => {
  test("default", async () => {
    const server = await createServer();
    expect(server).toBeDefined();
    expect(server.start).toBeDefined();
    expect(server.stop).toBeDefined();
  });
});
