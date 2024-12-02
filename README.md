# xcfx-node

Run the Conflux-Rust node in Node.js. This is used for developing on the Conflux network, such as testing RPC dApps or contracts.

## Supported platforms

By now xcfx-node supports the following platforms:

- Linux (x86_64-unknown-linux-gnu)
- MacOS (aarch64-apple-darwin and x86_64-apple-darwin)
- Windows (x86_64-pc-windows-msvc)

If you want to run other platform or architecture, you can open an [issue](https://github.com/iosh/xcfx-node/issues/new) on GitHub

## Mainnet and testnet

If you want to run a mainnet or testnet node, please refer to the [Conflux documentation](https://www.confluxdocs.com/docs/category/run-a-node)

## Quick start

### Install

```bash
npm install @xcfx/node
```

### Usage

```ts
import { createServer } from "@xcfx/node";
import { http, createPublicClient } from "cive";

async function main() {
  const server = await createServer({
    jsonrpcHttpPort: 12537,
  });

  await server.start();

  const client = createPublicClient({
    transport: http(`http://127.0.0.1:12537`),
  });

  const status = await client.getStatus();

  // safe to stop the server
  await server.stop();
}
await main();
```

### Configuration

```ts
import { createServer } from "@xcfx/node";

async function main() {
  const server = await createServer({ ...ConfluxConfig });
}
```

#### confluxConfig

```ts
export interface ConfluxConfig {
  /**
   * Set the node type to Full node, Archive node, or Light node.
   *  Possible values are "full", "archive",or"light".
   * @default "full"
   */
  nodeType?: string;
  /**
   * If it's not set, blocks will only be generated after receiving a transaction.Otherwise,
   * the blocks are automatically generated every
   */
  devBlockIntervalMs?: number;
  /** mining_author is the address to receive mining rewards. */
  miningAuthor?: string;
  /**
   * Listen address for stratum
   * @default "127.0.0.1"
   */
  stratumListenAddress?: string;
  /**
   * `mining_type` is the type of mining.
   * stratum | cpu | disable
   */
  miningType?: string;
  /** Port for stratum. */
  stratumPort?: number;
  /** log_conf` the path of the log4rs configuration file. The configuration in the file will overwrite the value set by `log_level`. */
  logConf?: string;
  /**
   * `log_file` is the path of the log file"
   * If not set, the log will only be printed to stdout, and not persisted to files.
   */
  logFile?: string;
  /**
   * log_level` is the printed log level.
   * "error" | "warn" | "info" | "debug" | "trace" | "off"
   */
  logLevel?: string;
  /**
   * The port of the websocket JSON-RPC server(public_rpc_apis is user defined).
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcWsPort?: number;
  /**
   * `tcp_port` is the TCP port that the process listens for P2P messages. The default is 32323.
   * @default 32323
   */
  tcpPort?: number;
  /**
   * `udp_port` is the UDP port used for node discovery.
   * @default 32323
   */
  udpPort?: number;
  /**
   * Possible Core space names are: all, safe, cfx, pos, debug, pubsub, test, trace, txpool.
   * `safe` only includes `cfx` and `pubsub`, `txpool`.
   *  @default "all"
   */
  publicRpcApis?: string;
  /**
   * Possible eSpace names are: eth, ethpubsub, ethdebug.
   *  @default 'evm'
   */
  publicEvmRpcApis?: string;
  /**
   * The chain ID of the network.(core space)
   * @default 1234
   */
  chainId?: number;
  /**
   * The chain ID of the network.( eSpace)
   *  @default 1235
   */
  evmChainId?: number;
  /**
   * The password used to encrypt the private key of the pos_private_key.
   *  @default "123456"
   */
  devPosPrivateKeyEncryptionPassword?: string;
  /**  The private key of the genesis (core space), every account will be receive 10000 CFX */
  genesisSecrets?: Array<string>;
  /**  The private key of the genesis (eSpace), every account will be receive 10000 CFX */
  genesisEvmSecrets?: Array<string>;
  /**
   * If it's `true`, `DEFERRED_STATE_EPOCH_COUNT` blocks are generated after
   * receiving a new tx through RPC calling to pack and execute this
   * transaction
   */
  devPackTxImmediately?: boolean;
  /** @default:1 */
  posReferenceEnableHeight?: number;
  /** @default:2 */
  defaultTransitionTime?: number;
  /** @default:3 */
  cip1559TransitionHeight?: number;
  /**
   * Enable CIP43A, CIP64, CIP71, CIP78A, CIP92 after hydra_transition_number
   * @default:1
   */
  hydraTransitionNumber?: number;
  /**
   * Enable cip76, cip86 after hydra_transition_height
   * @default:1
   */
  hydraTransitionHeight?: number;
  /** @default: temp dir */
  confluxDataDir?: string;
  /** pos config path */
  posConfigPath?: string;
  /** pos initial_nodes.json path */
  posInitialNodesPath?: string;
  /** pos pos_key file path */
  posPrivateKeyPath?: string;
  /**
   * Database type to store block-related data.
   * Supported: rocksdb, sqlite.
   * @default sqlite
   */
  blockDbType?: string;
  /** bootnodes is a list of nodes that a conflux node trusts, and will be used to sync the blockchain when a node starts. */
  bootnodes?: string;
  /** Window size for PoW manager */
  powProblemWindowSize?: number;
  /** # Secret key for stratum. The value is 64-digit hex string. If not set, the RPC subscription will not check the authorization. */
  stratumSecret?: string;
  /** `public_address` is the address of this node used */
  publicAddress?: string;
  /**
   * `jsonrpc_http_keep_alive` is used to control whether to set KeepAlive for rpc HTTP connections.
   * @default false
   */
  jsonrpcHttpKeepAlive?: boolean;
  /** `print_memory_usage_period_s` is the period for printing memory usage. */
  printMemoryUsagePeriodS?: number;
  /**
   * The port of the http JSON-RPC server.public_rpc_apis is user defined).
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcHttpPort?: number;
  /**
   * The port of the tcp JSON-RPC server. public_rpc_apis is user defined).
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcTcpPort?: number;
  /**
   * The port of the http JSON-RPC server public_rpc_apis is user defined).
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcHttpEthPort?: number;
  /**
   * The port of the websocket JSON-RPC serverpublic_rpc_apis is user defined).
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcWsEthPort?: number;
  /**
   * The port of the tcp JSON-RPC server(public_rpc_apis is "all").
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcLocalTcpPort?: number;
  /**
   * The port of the http JSON-RPC server(public_rpc_apis is "all").
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcLocalHttpPort?: number;
  /**
   * The port of the websocket JSON-RPC server(public_rpc_apis is "all").
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcLocalWsPort?: number;
  /**
   * `poll_lifetime_in_seconds` is the lifetime of the poll in seconds.
   * If set, the following RPC methods will be enabled:
   * - `cfx_newFilter` `cfx_newBlockFilter` `cfx_newPendingTransactionFilter` `cfx_getFilterChanges` `cfx_getFilterLogs` `cfx_uninstallFilter`.
   * - `eth_newFilter` `eth_newBlockFilter` `eth_newPendingTransactionFilter` eth_getFilterChanges eth_getFilterLogs eth_uninstallFilter
   */
  pollLifetimeInSeconds?: number;
  /** if `get_logs_filter_max_limit` is configured but the query would return more logs */
  getLogsFilterMaxLimit?: number;
}
```

### Example

The more example, please see `__test__` files.
