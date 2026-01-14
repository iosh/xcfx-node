import { createPublicClient, createTestClient, http as civeHttp } from "cive";
import { describe, expect, test } from "vitest";
import { createServer } from "../index";
import {
  getFreePorts,
  localChain,
  TEST_NETWORK_ID,
  TEST_PRIVATE_KEYS,
  wait,
} from "./help";

type EthTransactionReceipt = {
  status: string;
  contractAddress: string | null;
} & Record<string, unknown>;

describe("EVM Manual Block Generation", () => {
  test("should deploy a contract by manually mining enough blocks", async () => {
    // pk: 0x5674ac1fad4a1ce43e94917994c8f0c81140c4bbe807dbdc4945e0db5357f933
    // address: 0x79135a10B85be94eaA9bF23c21F2Eff3b055d498
    const EVM_DEPLOYER_PK =
      "5674ac1fad4a1ce43e94917994c8f0c81140c4bbe807dbdc4945e0db5357f933";
    const EVM_DEPLOY_RAW_TX =
      "0xf85780843b9aca00830f424080808560006000f382117fa09cb710949106adf559c101b9e0b654f2209a3979879072a367cd56f5e0e1149da001c063c38b46e1a058678359a650ea3cfb073b7e1da8839a7ae5e835fc8ee632";
    const EVM_DEPLOY_TX_HASH =
      "0x010777a2aff9f4e4f71471db837ebfa07c32c0dd667733c2a7df5abbd0be60a4";

    const [jsonrpcHttpPort, jsonrpcHttpEthPort, udpAndTcpPort] =
      await getFreePorts();

    const server = await createServer({
      nodeType: "full",
      jsonrpcHttpPort,
      jsonrpcHttpEthPort,
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,

      chainId: TEST_NETWORK_ID,
      evmChainId: 2222,
      genesisSecrets: TEST_PRIVATE_KEYS,
      genesisEvmSecrets: [...TEST_PRIVATE_KEYS, EVM_DEPLOYER_PK],

      devPackTxImmediately: false,
    });

    await server.start();

    try {
      const coreClient = createPublicClient({
        chain: localChain,
        transport: civeHttp(`http://127.0.0.1:${jsonrpcHttpPort}`),
      });

      const testClient = createTestClient({
        chain: localChain,
        transport: civeHttp(`http://127.0.0.1:${jsonrpcHttpPort}`),
      });

      const evmRpcUrl = `http://127.0.0.1:${jsonrpcHttpEthPort}`;
      const evmRpc = async <T>(method: string, params: unknown[] = []) => {
        const res = await fetch(evmRpcUrl, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ jsonrpc: "2.0", method, params, id: 1 }),
        }).then((r) => r.json());

        if (res.error) throw new Error(JSON.stringify(res.error));
        return res.result as T;
      };

      const txHash = await evmRpc<string>("eth_sendRawTransaction", [
        EVM_DEPLOY_RAW_TX,
      ]);
      expect(txHash).toBe(EVM_DEPLOY_TX_HASH);

      const receiptBefore = await evmRpc<unknown>("eth_getTransactionReceipt", [
        txHash,
      ]);
      expect(receiptBefore).toBeNull();

      const status0 = await coreClient.getStatus();

      // `mine({ blocks })` only generates *empty* blocks.
      // It advances height, but it does NOT pack pending transactions (including eSpace txs).
      await testClient.mine({ blocks: 5 });

      const statusAfterEmptyBlocks = await coreClient.getStatus();
      expect(statusAfterEmptyBlocks.blockNumber).toBe(status0.blockNumber + 5n);

      const receiptAfterEmptyBlocks = await evmRpc<unknown>(
        "eth_getTransactionReceipt",
        [txHash],
      );
      expect(receiptAfterEmptyBlocks).toBeNull();

      // `mine({ numTxs })` generates blocks that try to pack transactions from the txpool.
      // Internally it calls `test_generateOneBlock` `deferredStateEpochCount` times (defaults to 5),
      // so *each* `mine({ numTxs })` call generates 5 blocks.
      //
      // For eSpace, the receipt may not show up immediately. Mine one packed batch first,
      // then keep generating empty blocks until the receipt is available.
      await testClient.mine({ numTxs: 1 });
      const statusAfterPackedBlocks = await coreClient.getStatus();
      expect(statusAfterPackedBlocks.blockNumber).toBe(
        statusAfterEmptyBlocks.blockNumber + 5n,
      );

      let receipt: EthTransactionReceipt | null = null;
      for (let i = 0; i < 30; i++) {
        receipt = await evmRpc<EthTransactionReceipt | null>(
          "eth_getTransactionReceipt",
          [txHash],
        );
        if (receipt) break;
        // Generate an extra empty block to advance epochs for deferred execution.
        await testClient.mine({ blocks: 1 });
        await wait(200);
      }

      if (!receipt) {
        throw new Error("Transaction was not mined after generating blocks");
      }

      expect(receipt.status).toBe("0x1");
      expect(receipt.contractAddress).not.toBeNull();
      expect(receipt.contractAddress).toMatch(/^0x[0-9a-fA-F]{40}$/);
    } finally {
      await server.stop();
    }
  }, 60_000);
});
