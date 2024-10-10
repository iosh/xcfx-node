/* auto-generated by NAPI-RS */
/* eslint-disable */
export declare class ConfluxNode {
  constructor()
  startNode(config: ConfluxConfig): void
  stopNode(): void
}

export interface ConfluxConfig {
  /**
   * Set the node type to Full node, Archive node, or Light node.
   *  Possible values are "full", "archive",or"light".
   * @default "full"
   */
  nodeType?: string
  /**
   * If it's not set, blocks will only be generated after receiving a transaction.Otherwise,
   * the blocks are automatically generated every
   */
  devBlockIntervalMs?: number
  /** mining_author is the address to receive mining rewards. */
  miningAuthor?: string
  /**
   * Listen address for stratum
   * @default "127.0.0.1"
   */
  stratumListenAddress?: string
  /**
   * Port for stratum.
   * @default 32525
   */
  stratumPort?: number
  /** log_conf` the path of the log4rs configuration file. The configuration in the file will overwrite the value set by `log_level`. */
  logConf?: string
  /**
   * `log_file` is the path of the log file"
   * If not set, the log will only be printed to stdout, and not persisted to files.
   */
  logFile?: string
  /**
   * log_level` is the printed log level.
   * "error" | "warn" | "info" | "debug" | "trace" | "off"
   */
  logLevel?: string
  /**
   * The port of the websocket JSON-RPC server.
   *  @default 12535
   */
  jsonrpcWsPort?: number
  /**
   * The port of the HTTP JSON-RPC server.
   * @default 12537
   */
  jsonrpcHttpPort?: number
  /**
   * Possible Core space names are: all, safe, cfx, pos, debug, pubsub, test, trace, txpool.
   * `safe` only includes `cfx` and `pubsub`, `txpool`.
   *  @default "all"
   */
  publicRpcApis?: string
  /**
   * The chain ID of the network.(core space)
   * @default 1234
   */
  chainId?: number
  /**
   * The chain ID of the network.( eSpace)
   *  @default 1235
   */
  evmChainId?: number
  /**
   * The password used to encrypt the private key of the pos_private_key.
   *  @default "123456"
   */
  devPosPrivateKeyEncryptionPassword?: string
  /**  The private key of the genesis, every account will be receive 1000 CFX */
  genesisSecrets?: Array<string>
  /**  @default: false */
  devPackTxImmediately?: boolean
  /** @default:1 */
  posReferenceEnableHeight?: number
  /** @default:2 */
  defaultTransitionTime?: number
  /** @default:3 */
  cip1559TransitionHeight?: number
  /** @default: temp dir */
  confluxDataDir?: string
  /** pos config path */
  posConfigPath?: string
  /** pos initial_nodes.json path */
  posInitialNodesPath?: string
  /** pos pos_key file path */
  posPrivateKeyPath?: string
  /**
   * Database type to store block-related data.
   * Supported: rocksdb, sqlite.
   * @default sqlite
   */
  blockDbType?: string
}

