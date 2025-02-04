# xcfx-node

Run a Conflux-Rust node in Node.js for development and testing purposes. Perfect for testing RPC dApps or smart contracts.

## Features

- 🚀 Easy to set up and use
- 🔧 Highly configurable
- 💻 Cross-platform support
- 🔌 Built-in JSON-RPC server (HTTP & WebSocket)
- 📦 Automatic & Manual block generation
- 🔍 Customizable logging
- 💎 Support for both Core space and EVM space

## Supported Platforms

- Linux (x86_64-unknown-linux-gnu)
- MacOS (aarch64-apple-darwin, x86_64-apple-darwin)
- Windows (x86_64-pc-windows-msvc)

Need support for another platform? [Open an issue](https://github.com/iosh/xcfx-node/issues/new)

## Installation

```bash
npm install xcfx-node
# or
yarn add xcfx-node
# or
pnpm add xcfx-node
```

## Basic Usage

Here's a simple example to get you started:

```ts
import { createServer } from "@xcfx/node";
import { http, createPublicClient } from "cive";

async function main() {
  // Create a development node
  const server = await createServer({
    jsonrpcHttpPort: 12537,
    // Enable automatic block generation every 1 second
    devBlockIntervalMs: 1000,
  });

  // Start the node
  await server.start();

  // Create a client to interact with the node
  const client = createPublicClient({
    transport: http(`http://127.0.0.1:12537`),
  });

  // Query node status
  const status = await client.getStatus();
  console.log("Node status:", status);

  // Stop the node when done
  await server.stop();
}

main().catch(console.error);
```

## Common Use Cases

### 1. Automatic Block Generation

```ts
// Create a node with automatic block generation
const server = await createServer({
  jsonrpcHttpPort: 12537,
  devBlockIntervalMs: 100, // Generate blocks every 100ms
});
```

### 2. Manual Block Generation

```ts
import { createTestClient } from "cive";

// Create a node with manual block generation
const server = await createServer({
  jsonrpcHttpPort: 12537,
  devPackTxImmediately: false, // Disable automatic block generation
});

// Use test client to generate blocks manually
const testClient = createTestClient({
  transport: http(`http://127.0.0.1:12537`),
});

// Generate 10 blocks
await testClient.mine({ blocks: 10 });
```

### 3. Custom Logging Configuration

```ts
// Use default console logging
const server1 = await createServer({
  jsonrpcHttpPort: 12537,
  log: true, // Enable default console logging
});

// Use custom log configuration
const server2 = await createServer({
  jsonrpcHttpPort: 12537,
  logConf: "./path/to/your/log.yaml", // Custom log configuration, you can use the default log configuration file in configs/log.yaml
});
```

### 4. Genesis Account Configuration

```ts
// Configure initial accounts in both spaces
const server = await createServer({
  jsonrpcHttpPort: 12537,
  jsonrpcHttpEthPort: 8545,
  chainId: 1111,          // Core space chain ID
  evmChainId: 2222,       // EVM space chain ID
  genesisSecrets: [       // Core space accounts
    "0x.....",
    "0x....."
  ],
  genesisEvmSecrets: [    // EVM space accounts
    "0x.....",
    "0x....."
  ],
});
```

## Advanced Configuration

The `createServer` function accepts various configuration options:

```ts
interface ConfluxConfig {
  // Node Type Configuration
  nodeType?: "full" | "archive" | "light"; // default: "full"
  blockDbType?: "rocksdb" | "sqlite";      // default: "sqlite"
  
  // Data Storage
  confluxDataDir?: string;        // Data directory, default: temp dir
  
  // Network Configuration
  chainId?: number;        // Core space chain ID, default: 1234
  evmChainId?: number;     // EVM space chain ID, default: 1235
  bootnodes?: string;      // Bootstrap nodes list
  
  // Mining Configuration
  miningAuthor?: string;   // Mining rewards recipient address
  miningType?: "stratum" | "cpu" | "disable";
  stratumListenAddress?: string;  // default: "127.0.0.1"
  stratumPort?: number;
  stratumSecret?: string;
  
  // Development Options
  devBlockIntervalMs?: number;     // Automatic block generation interval
  devPackTxImmediately?: boolean;  // Pack transactions immediately
  
  // Logging
  log?: boolean;           // Enable console logging
  logConf?: string;        // Custom log configuration file path
  
  // RPC Endpoints
  jsonrpcHttpPort?: number;  // HTTP RPC port
  jsonrpcWsPort?: number;    // WebSocket RPC port
}
```

## For Production Use

For running nodes in production (mainnet or testnet), please refer to the [official Conflux documentation](https://www.confluxdocs.com/docs/category/run-a-node).

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

MIT

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
interface ConfluxConfig {
  /**
   * Set the node type to Full node, Archive node, or Light node.
   *  Possible values are "full", "archive",or "light".
   * @default "full"
   */
  nodeType?: string;
  /**
   * Database type to store block-related data.
   * Supported: rocksdb, sqlite.
   * @default sqlite
   */
  blockDbType?: string;
  /** @default: data_dir */
  confluxDataDir?: string;
  /**
   * Add data directory configuration
   * The conflux node will use this directory to store data.  If not set, a temporary directory will be used.
   * @default temp dir
   */
  dataDir?: string;
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
  /** bootnodes is a list of nodes that a conflux node trusts, and will be used to sync the blockchain when a node starts. */
  bootnodes?: string;
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
  /** Secret key for stratum. The value is 64-digit hex string. If not set, the RPC subscription will not check the authorization. */
  stratumSecret?: string;
  /** Window size for PoW manager */
  powProblemWindowSize?: number;
  /**
   * If it's not set, blocks will only be generated after receiving a transaction.Otherwise,
   * the blocks are automatically generated every
   */
  devBlockIntervalMs?: number;
  /**
   * If it's `true`, `DEFERRED_STATE_EPOCH_COUNT` blocks are generated after
   * receiving a new tx through RPC calling to pack and execute this
   * transaction
   */
  devPackTxImmediately?: boolean;
  /**  The private key of the genesis (core space), every account will be receive 10000 CFX */
  genesisSecrets?: Array<string>;
  /**  The private key of the genesis (eSpace), every account will be receive 10000 CFX */
  genesisEvmSecrets?: Array<string>;
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
  /** `public_address` is the address of this node used */
  publicAddress?: string;
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
   * The port of the websocket JSON-RPC server(public_rpc_apis is user defined).
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcWsPort?: number;
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
   * `jsonrpc_http_keep_alive` is used to control whether to set KeepAlive for rpc HTTP connections.
   * @default false
   */
  jsonrpcHttpKeepAlive?: boolean;
  /**
   * The password used to encrypt the private key of the pos_private_key.
   *  @default "123456"
   */
  devPosPrivateKeyEncryptionPassword?: string;
  /** @default:1 */
  posReferenceEnableHeight?: number;
  /** pos config path */
  posConfigPath?: string;
  /** pos initial_nodes.json path */
  posInitialNodesPath?: string;
  /** pos pos_key file path */
  posPrivateKeyPath?: string;
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
  /**
   * Enable cip112 after hydra_transition_height
   * @default:1
   */
  cip112TransitionHeight?: number;
  /** log_conf` the path of the log4rs configuration file. The configuration in the file will overwrite the value set by `log_level`. */
  logConf?: string;
  /**
   * log_level` is the printed log level.
   * "error" | "warn" | "info" | "debug" | "trace" | "off"
   */
  logLevel?: string;
  /** `print_memory_usage_period_s` is the period for printing memory usage. */
  printMemoryUsagePeriodS?: number;
  /**
   * `poll_lifetime_in_seconds` is the lifetime of the poll in seconds.
   * If set, the following RPC methods will be enabled:
   * - `cfx_newFilter` `cfx_newBlockFilter` `cfx_newPendingTransactionFilter` `cfx_getFilterChanges` `cfx_getFilterLogs` `cfx_uninstallFilter`.
   * - `eth_newFilter` `eth_newBlockFilter` `eth_newPendingTransactionFilter` eth_getFilterChanges eth_getFilterLogs eth_uninstallFilter
   * @default: 60
   */
  pollLifetimeInSeconds?: number;
  /** if `get_logs_filter_max_limit` is configured but the query would return more logs */
  getLogsFilterMaxLimit?: number;
}
```

### Example

The more example, please see `__test__` files.
