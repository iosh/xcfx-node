import { defineConfig } from "vitest/config";
import { BaseSequencer } from "vitest/node";
export default defineConfig({
  test: {
    testTimeout: 500000,
    hookTimeout: 500000,
    environment: "node",
  },
});
