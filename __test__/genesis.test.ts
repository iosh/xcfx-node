import { http, createPublicClient, parseCFX } from "cive";
import { privateKeyToAccount } from "cive/accounts";
import { base32AddressToHex } from "cive/utils";
import { describe, expect, test } from "vitest";
import { createServer } from "../index";
import { TEST_NETWORK_ID, TEST_PK, getFreePorts } from "./help";

describe("genesis", () => {
  test("default", async () => {
    const [jsonrpcHttpPort, jsonrpcHttpEthPort, udpAndTcpPort] =
      await getFreePorts();
    const server = await createServer({
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,
      chainId: TEST_NETWORK_ID,
      jsonrpcHttpPort: jsonrpcHttpPort,
      jsonrpcHttpEthPort: jsonrpcHttpEthPort,
      genesisSecrets: TEST_PK,
      // 0x prefix is optional
      genesisEvmSecrets: TEST_PK.map((pk) => `0x${pk}`),
    });
    await server.start();
    const client = createPublicClient({
      transport: http(`http://127.0.0.1:${jsonrpcHttpPort}`),
    });

    const coreSpaceAccount = privateKeyToAccount(`0x${TEST_PK[0]}`, {
      networkId: TEST_NETWORK_ID,
    });

    const balance = await client.getBalance({
      address: coreSpaceAccount.address,
    });

    expect(balance).toBe(10000000000000000000000n);
    const evmBalance = await fetch(`http://127.0.0.1:${jsonrpcHttpEthPort}`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        method: "eth_getBalance",
        params: [
          base32AddressToHex({ address: coreSpaceAccount.address }),
          "latest",
        ],
        id: 1,
      }),
    }).then((res) => res.json());

    expect(BigInt(evmBalance.result)).toBe(10000000000000000000000n);

    await server.stop();
  });
});
