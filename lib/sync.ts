import { createTestClient, http, webSocket } from "cive";
import type { NodeRequestOptions } from "./types";

const withTimeout = async <T>(
  promise: Promise<T>,
  timeoutMs: number,
  timeoutMessage: string,
): Promise<T> => {
  if (timeoutMs <= 0) {
    throw new Error(timeoutMessage);
  }
  let timeoutId: ReturnType<typeof setTimeout> | undefined;
  const timeoutPromise = new Promise<T>((_resolve, reject) => {
    timeoutId = setTimeout(() => reject(new Error(timeoutMessage)), timeoutMs);
  });
  try {
    return await Promise.race([promise, timeoutPromise]);
  } finally {
    if (timeoutId) clearTimeout(timeoutId);
  }
};

export const waitForNodeRPCReady = async (config: NodeRequestOptions) => {
  // if no ports are provided, return because there is no rpc to setup
  if (!config.httpPort && !config.wsPort) return;

  const transport = config.httpPort
    ? http(`http://127.0.0.1:${config.httpPort}`)
    : webSocket(`ws://127.0.0.1:${config.wsPort}`);

  const testClient = createTestClient({ transport });

  const startTime = Date.now();
  const deadline = startTime + config.timeout;

  while (Date.now() < deadline) {
    const remainingMs = deadline - Date.now();
    const perAttemptTimeoutMs = Math.min(2000, remainingMs);

    try {
      const phase = await withTimeout(
        testClient.getCurrentSyncPhase(),
        perAttemptTimeoutMs,
        "Get node sync phase timeout",
      );
      if (phase === "NormalSyncPhase") {
        return;
      }
    } catch (_error) {
      // Ignore transient errors (connection refused, startup churn, per-attempt timeout)
      // and keep retrying until the overall timeout is reached.
    }

    await new Promise((resolve) =>
      setTimeout(
        resolve,
        Math.min(config.retryInterval, deadline - Date.now()),
      ),
    );
  }

  throw new Error("Get node sync phase timeout");
};
