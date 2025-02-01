/* auto-generated by NAPI-RS */
/* eslint-disable */
export declare class ConfluxNode {
  constructor()
  startNode(config: ConfluxConfig, jsCallback: ((err: Error | null, arg: null) => any)): void
  stopNode(jsCallback: ((err: Error | null, arg: null) => any)): void
}

export interface ConfluxConfig {
  /**
   * Set the node type to Full node, Archive node, or Light node.
   *  Possible values are "full", "archive",or "light".
   * @default "full"
   */
  nodeType?: string
  /**
   * Database type to store block-related data.
   * Supported: rocksdb, sqlite.
   * @default sqlite
   */
  blockDbType?: string
  /** @default: data_dir */
  confluxDataDir?: string
  /**
   * Add data directory configuration
   * The conflux node will use this directory to store data.  If not set, a temporary directory will be used.
   * @default temp dir
   */
  dataDir?: string
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
  /** bootnodes is a list of nodes that a conflux node trusts, and will be used to sync the blockchain when a node starts. */
  bootnodes?: string
  /** mining_author is the address to receive mining rewards. */
  miningAuthor?: string
  /**
   * Listen address for stratum
   * @default "127.0.0.1"
   */
  stratumListenAddress?: string
  /**
   * `mining_type` is the type of mining.
   * stratum | cpu | disable
   */
  miningType?: string
  /** Port for stratum. */
  stratumPort?: number
  /** Secret key for stratum. The value is 64-digit hex string. If not set, the RPC subscription will not check the authorization. */
  stratumSecret?: string
  /** Window size for PoW manager */
  powProblemWindowSize?: number
  /**
   * If it's not set, blocks will only be generated after receiving a transaction.Otherwise,
   * the blocks are automatically generated every
   */
  devBlockIntervalMs?: number
  /**
   * If it's `true`, `DEFERRED_STATE_EPOCH_COUNT` blocks are generated after
   * receiving a new tx through RPC calling to pack and execute this
   * transaction
   */
  devPackTxImmediately?: boolean
  /**  The private key of the genesis (core space), every account will be receive 10000 CFX */
  genesisSecrets?: Array<string>
  /**  The private key of the genesis (eSpace), every account will be receive 10000 CFX */
  genesisEvmSecrets?: Array<string>
  /**
   * `tcp_port` is the TCP port that the process listens for P2P messages. The default is 32323.
   * @default 32323
   */
  tcpPort?: number
  /**
   * `udp_port` is the UDP port used for node discovery.
   * @default 32323
   */
  udpPort?: number
  /** `public_address` is the address of this node used */
  publicAddress?: string
  /**
   * Possible Core space names are: all, safe, cfx, pos, debug, pubsub, test, trace, txpool.
   * `safe` only includes `cfx` and `pubsub`, `txpool`.
   *  @default "all"
   */
  publicRpcApis?: string
  /**
   * Possible eSpace names are: eth, ethpubsub, ethdebug.
   *  @default 'evm'
   */
  publicEvmRpcApis?: string
  /**
   * The port of the websocket JSON-RPC server(public_rpc_apis is user defined).
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcWsPort?: number
  /**
   * The port of the http JSON-RPC server.public_rpc_apis is user defined).
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcHttpPort?: number
  /**
   * The port of the tcp JSON-RPC server. public_rpc_apis is user defined).
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcTcpPort?: number
  /**
   * The port of the http JSON-RPC server public_rpc_apis is user defined).
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcHttpEthPort?: number
  /**
   * The port of the websocket JSON-RPC serverpublic_rpc_apis is user defined).
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcWsEthPort?: number
  /**
   * The port of the tcp JSON-RPC server(public_rpc_apis is "all").
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcLocalTcpPort?: number
  /**
   * The port of the http JSON-RPC server(public_rpc_apis is "all").
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcLocalHttpPort?: number
  /**
   * The port of the websocket JSON-RPC server(public_rpc_apis is "all").
   * if not set, the JSON-RPC server will not be started.
   * @default null
   */
  jsonrpcLocalWsPort?: number
  /**
   * `jsonrpc_http_keep_alive` is used to control whether to set KeepAlive for rpc HTTP connections.
   * @default false
   */
  jsonrpcHttpKeepAlive?: boolean
  /**
   * The password used to encrypt the private key of the pos_private_key.
   *  @default "123456"
   */
  devPosPrivateKeyEncryptionPassword?: string
  /** @default:1 */
  posReferenceEnableHeight?: number
  /** pos config path */
  posConfigPath?: string
  /** pos initial_nodes.json path */
  posInitialNodesPath?: string
  /** pos pos_key file path */
  posPrivateKeyPath?: string
  /** @default:2 */
  defaultTransitionTime?: number
  /** @default:3 */
  cip1559TransitionHeight?: number
  /**
   * Enable CIP43A, CIP64, CIP71, CIP78A, CIP92 after hydra_transition_number
   * @default:1
   */
  hydraTransitionNumber?: number
  /**
   * Enable cip76, cip86 after hydra_transition_height
   * @default:1
   */
  hydraTransitionHeight?: number
  /** log_conf` the path of the log4rs configuration file. The configuration in the file will overwrite the value set by `log_level`. */
  logConf?: string
  /**
   * log_level` is the printed log level.
   * "error" | "warn" | "info" | "debug" | "trace" | "off"
   */
  logLevel?: string
  /** `print_memory_usage_period_s` is the period for printing memory usage. */
  printMemoryUsagePeriodS?: number
  /**
   * `poll_lifetime_in_seconds` is the lifetime of the poll in seconds.
   * If set, the following RPC methods will be enabled:
   * - `cfx_newFilter` `cfx_newBlockFilter` `cfx_newPendingTransactionFilter` `cfx_getFilterChanges` `cfx_getFilterLogs` `cfx_uninstallFilter`.
   * - `eth_newFilter` `eth_newBlockFilter` `eth_newPendingTransactionFilter` eth_getFilterChanges eth_getFilterLogs eth_uninstallFilter
   * @default: 60
   */
  pollLifetimeInSeconds?: number
  /** if `get_logs_filter_max_limit` is configured but the query would return more logs */
  getLogsFilterMaxLimit?: number
}

