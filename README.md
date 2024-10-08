# xcfx-node

Run the Conflux-Rust node in Node.js. This is used for developing on the Conflux network, such as testing RPC dApps or contracts.

# Supported platforms

By now xcfx-node supports the following platforms:

- Linux (x86_64-unknown-linux-gnu)
- MacOS (aarch64-apple-darwin and x86_64-apple-darwin)
- Windows (x86_64-pc-windows-msvc)

If you want to run other platform or architecture, you can open an [issue](https://github.com/iosh/xcfx-node/issues/new) on GitHub

# Mainnet and testnet

If you want to run a mainnet or testnet node, please refer to the [Conflux documentation](https://www.confluxdocs.com/docs/category/run-a-node)

# Quick start

## Install

```bash
npm install @xcfx/node
```

## Usage

```ts
import { createServer } from "@xcfx/node";

async function main() {
  const server = await createServer();

  await server.start();

  const options = {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: '{"jsonrpc":"2.0","method":"cfx_getStatus","id":1}',
  };

  const response = await fetch("http://localhost:12537", options);
  const data = await response.json();

  // safe to stop the server
  await server.stop();
}
await main();
```

## Configuration

```ts
import { createServer } from "@xcfx/node";

async function main() {
  const server = await createServer({ ...serverConfig, ...ConfluxConfig });
}
```

### confluxConfig

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
   * Port for stratum.
   * @default 32525
   */
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
   * The port of the websocket JSON-RPC server.
   *  @default 12535
   */
  jsonrpcWsPort?: number;
  /**
   * The port of the HTTP JSON-RPC server.
   * @default 12537
   */
  jsonrpcHttpPort?: number;
  /**
   * Possible Core space names are: all, safe, cfx, pos, debug, pubsub, test, trace, txpool.
   * `safe` only includes `cfx` and `pubsub`, `txpool`.
   *  @default "all"
   */
  publicRpcApis?: string;
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
  /**  The private key of the genesis, every account will be receive 1000 CFX */
  genesisSecrets?: Array<string>;
  /**  @default: false */
  devPackTxImmediately?: boolean;
  /** @default:0 */
  posReferenceEnableHeight?: number;
  /** @default: 1 */
  defaultTransitionTime?: number;
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
}
```
