# xcfx-node

Run the Conflux-Rust node in Node.js. This is used for developing on the Conflux network, such as testing RPC dApps or contracts.

# Supported platforms

By now xcfx-node supports the following platforms:

- Linux (x86_64-unknown-linux-gnu)
- macOS (aarch64-apple-darwin)

If you want to run other platform or architecture, you can open an [issue](https://github.com/iosh/xcfx-node/issues/new) on GitHub

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

### serverConfig

```ts
export interface ServerConfig {
  /**
   * wait until the conflux node is ready
   * @default true
   */
  waitUntilReady?: true;

  /**
   * Print the conflux node output to the console.log
   * @default true
   */
  silent?: boolean;

  /**
   * Persist the conflux node data, if not set, the data will be deleted when the conflux node is stopped
   * @default false
   */
  persistNodeData?: boolean;
}
```

### confluxConfig

```ts
export interface ConfluxConfig {
  /**Set the node type to Full node, Archive node, or Light node.
   * Possible values are "full", "archive",or"light".
   * @default "full"
   */
  node_type?: "full" | "archive" | "light";

  /**
   * `dev` mode is for users to run a single node that automatically
   * @default "dev"
   */
  mode?: "dev";

  /**
   * If it's not set, blocks will only be generated after receiving a transaction.Otherwise,
   * the blocks are automatically generated every
   */
  dev_block_interval_ms?: number;

  /**
   * `mining_author` is the address to receive mining rewards.
   */
  mining_author?: string;

  /**
   * Listen address for stratum
   * @default "127.0.0.1"
   */
  stratum_listen_address?: string;

  /**
   * Port for stratum.
   * @default 32525
   */

  stratum_port?: number;

  /**
   * `log_conf` the path of the log4rs configuration file. The configuration in the file will overwrite the value set by `log_level`.
   */
  log_conf?: string;
  /**
   *`log_file` is the path of the log file"
   * If not set, the log will only be printed to stdout, and not persisted to files.
   */
  log_file?: string;

  /**
   * log_level` is the printed log level.
   */

  log_level?: "error" | "warn" | "info" | "debug" | "trace" | "off";

  /**
   * The port of the websocket JSON-RPC server.
   * @default 12535
   */
  jsonrpc_ws_port?: number;

  /**
   * The port of the HTTP JSON-RPC server.
   * @default 12537
   */
  jsonrpc_http_port?: number;
  /**
   * Possible Core space names are: all, safe, cfx, pos, debug, pubsub, test, trace, txpool.
   * `safe` only includes `cfx` and `pubsub`, `txpool`.
   * @default "all"
   */
  public_rpc_apis?:
    | "all"
    | "safe"
    | "cfx"
    | "pos"
    | "debug"
    | "pubsub"
    | "test"
    | "trace"
    | "txpool";

  /**
   * The chain ID of the network.(core space)
   * @default 1234
   */
  chain_id?: number;

  /**
   * The chain ID of the network.( eSpace)
   * @default 1235
   */
  evm_chain_id?: number;

  /**
   * The password used to encrypt the private key of the pos_private_key.
   * @default "123456"
   */
  dev_pos_private_key_encryption_password?: string;

  /**
   * The private key of the genesis, every account will be receive 1000 CFX
   */
  genesis_secrets?: string[];

  /**
   * To open filter related methods
   */
  poll_lifetime_in_seconds?: number;

  /**
   * @default 0
   */
  pos_reference_enable_height?: number;

  /**
   * @default 1
   */
  dao_vote_transition_number?: number;

  /**
   * @default 1
   */
  dao_vote_transition_height?: number;

  /**
   * @default 1
   */
  cip43_init_end_number?: number;
  /**
   * @default 1
   */
  cip71_patch_transition_number?: number;
  /**
   * @default 1
   */
  cip90_transition_height?: number;
  /**
   * @default 1
   */
  cip90_transition_number?: number;

  /**
   * @default 1
   */
  cip105_transition_number?: number;
  /**
   * @default 1
   */
  cip107_transition_number?: number;
  /**
   * @default 1
   */
  cip112_transition_height?: number;
  /**
   * @default 1
   */
  cip118_transition_number?: number;
  /**
   * @default 1
   */
  cip119_transition_number?: number;

  /**
   * @default 1
   */
  next_hardfork_transition_number?: number;
  /**
   * @default 1
   */
  next_hardfork_transition_height?: number;
  /**
   * @default 1
   */
  cip1559_transition_height?: number;
  /**
   * @default 1
   */
  cancun_opcodes_transition_number?: number;
  /**
   * @default 1
   */
  cip113_transition_height?: number;
}
```
