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

### 5. Using Configuration File

```ts
const server = await createServer({
  configFile: "./path/to/config.toml",
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

  // Configuration File
  configFile?: string;       // Path to configuration file
}
```
## Example

The more example, please see `__test__` files.

## For Production Use

For running nodes in production (mainnet or testnet), please refer to the [official Conflux documentation](https://www.confluxdocs.com/docs/category/run-a-node).

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

MIT
