use cfxcore::NodeType;
use client::{common::Configuration, rpc::rpc_apis::ApiSet};
use napi_derive::napi;
use primitives::block_header::CIP112_TRANSITION_HEIGHT;
use std::{
  fs::File,
  io::{BufWriter, Write},
  path::Path,
  str::FromStr,
};

#[napi(object)]
#[derive(Debug)]
pub struct ConfluxConfig {
  // ============= Node Configuration =============
  /// Set the node type to Full node, Archive node, or Light node.
  ///  Possible values are "full", "archive",or "light".
  /// @default "full"
  pub node_type: Option<String>,

  /// Database type to store block-related data.
  /// Supported: rocksdb, sqlite.
  /// @default sqlite
  pub block_db_type: Option<String>,

  /// @default: data_dir
  pub conflux_data_dir: Option<String>,

  /// Add data directory configuration
  /// The conflux node will use this directory to store data.  If not set, a temporary directory will be used.
  /// @default temp dir
  pub data_dir: Option<String>,

  // ============= Chain Configuration =============
  /// The chain ID of the network.(core space)
  /// @default 1234
  pub chain_id: Option<u32>,

  /// The chain ID of the network.( eSpace)
  ///  @default 1235
  pub evm_chain_id: Option<u32>,

  /// bootnodes is a list of nodes that a conflux node trusts, and will be used to sync the blockchain when a node starts.
  pub bootnodes: Option<String>,

  // ============= Mining Configuration =============
  /// mining_author is the address to receive mining rewards.
  pub mining_author: Option<String>,

  /// Listen address for stratum
  /// @default "127.0.0.1"
  pub stratum_listen_address: Option<String>,

  /// `mining_type` is the type of mining.
  /// stratum | cpu | disable
  pub mining_type: Option<String>,

  /// Port for stratum.
  pub stratum_port: Option<u16>,

  /// Secret key for stratum. The value is 64-digit hex string. If not set, the RPC subscription will not check the authorization.
  pub stratum_secret: Option<String>,

  /// Window size for PoW manager
  pub pow_problem_window_size: Option<u32>,

  // ============= Development Mode Configuration =============
  /// If it's not set, blocks will only be generated after receiving a transaction.Otherwise,
  /// the blocks are automatically generated every
  pub dev_block_interval_ms: Option<i64>,

  /// If it's `true`, `DEFERRED_STATE_EPOCH_COUNT` blocks are generated after
  /// receiving a new tx through RPC calling to pack and execute this
  /// transaction
  pub dev_pack_tx_immediately: Option<bool>,

  ///  The private key of the genesis (core space), every account will be receive 10000 CFX
  pub genesis_secrets: Option<Vec<String>>,

  ///  The private key of the genesis (eSpace), every account will be receive 10000 CFX
  pub genesis_evm_secrets: Option<Vec<String>>,

  // ============= Network Configuration =============
  /// `tcp_port` is the TCP port that the process listens for P2P messages. The default is 32323.
  /// @default 32323
  pub tcp_port: Option<u16>,

  /// `udp_port` is the UDP port used for node discovery.
  /// @default 32323
  pub udp_port: Option<u16>,

  /// `public_address` is the address of this node used
  pub public_address: Option<String>,

  // ============= JSON-RPC Configuration =============
  /// Possible Core space names are: all, safe, cfx, pos, debug, pubsub, test, trace, txpool.
  /// `safe` only includes `cfx` and `pubsub`, `txpool`.
  ///  @default "all"
  pub public_rpc_apis: Option<String>,

  /// Possible eSpace names are: eth, ethpubsub, ethdebug.
  ///  @default 'evm'
  pub public_evm_rpc_apis: Option<String>,

  /// The port of the websocket JSON-RPC server(public_rpc_apis is user defined).
  /// if not set, the JSON-RPC server will not be started.
  /// @default null
  pub jsonrpc_ws_port: Option<u16>,

  /// The port of the http JSON-RPC server.public_rpc_apis is user defined).
  /// if not set, the JSON-RPC server will not be started.
  /// @default null
  pub jsonrpc_http_port: Option<u16>,

  /// The port of the tcp JSON-RPC server. public_rpc_apis is user defined).
  /// if not set, the JSON-RPC server will not be started.
  /// @default null
  pub jsonrpc_tcp_port: Option<u16>,

  /// The port of the http JSON-RPC server public_rpc_apis is user defined).
  /// if not set, the JSON-RPC server will not be started.
  /// @default null
  pub jsonrpc_http_eth_port: Option<u16>,

  /// The port of the websocket JSON-RPC serverpublic_rpc_apis is user defined).
  /// if not set, the JSON-RPC server will not be started.
  /// @default null
  pub jsonrpc_ws_eth_port: Option<u16>,

  /// The port of the tcp JSON-RPC server(public_rpc_apis is "all").
  /// if not set, the JSON-RPC server will not be started.
  /// @default null
  pub jsonrpc_local_tcp_port: Option<u16>,

  /// The port of the http JSON-RPC server(public_rpc_apis is "all").
  /// if not set, the JSON-RPC server will not be started.
  /// @default null
  pub jsonrpc_local_http_port: Option<u16>,

  /// The port of the websocket JSON-RPC server(public_rpc_apis is "all").
  /// if not set, the JSON-RPC server will not be started.
  /// @default null
  pub jsonrpc_local_ws_port: Option<u16>,

  /// `jsonrpc_http_keep_alive` is used to control whether to set KeepAlive for rpc HTTP connections.
  /// @default false
  pub jsonrpc_http_keep_alive: Option<bool>,

  // ============= PoS Configuration =============
  /// The password used to encrypt the private key of the pos_private_key.
  ///  @default "123456"
  pub dev_pos_private_key_encryption_password: Option<String>,

  /// @default:1
  pub pos_reference_enable_height: Option<i64>,

  /// pos config path
  pub pos_config_path: Option<String>,

  /// pos initial_nodes.json path
  pub pos_initial_nodes_path: Option<String>,

  /// pos pos_key file path
  pub pos_private_key_path: Option<String>,

  // ============= Protocol Upgrade Configuration =============
  /// @default:2
  pub default_transition_time: Option<i64>,

  /// @default:3
  pub cip1559_transition_height: Option<i64>,

  /// Enable CIP43A, CIP64, CIP71, CIP78A, CIP92 after hydra_transition_number
  /// @default:1
  pub hydra_transition_number: Option<i64>,

  /// Enable cip76, cip86 after hydra_transition_height
  /// @default:1
  pub hydra_transition_height: Option<i64>,

  /// Enable cip112 after hydra_transition_height
  /// @default:1
  pub cip112_transition_height: Option<i64>,

  // ============= Logging Configuration =============
  /// log_conf` the path of the log4rs configuration file. The configuration in the file will overwrite the value set by `log_level`.
  pub log_conf: Option<String>,

  /// log_level` is the printed log level.
  /// "error" | "warn" | "info" | "debug" | "trace" | "off"
  pub log_level: Option<String>,

  /// `print_memory_usage_period_s` is the period for printing memory usage.
  pub print_memory_usage_period_s: Option<u32>,

  // ============= Filter and Poll Configuration =============
  /// `poll_lifetime_in_seconds` is the lifetime of the poll in seconds.
  /// If set, the following RPC methods will be enabled:
  /// - `cfx_newFilter` `cfx_newBlockFilter` `cfx_newPendingTransactionFilter` `cfx_getFilterChanges` `cfx_getFilterLogs` `cfx_uninstallFilter`.
  /// - `eth_newFilter` `eth_newBlockFilter` `eth_newPendingTransactionFilter` eth_getFilterChanges eth_getFilterLogs eth_uninstallFilter
  /// @default: 60
  pub poll_lifetime_in_seconds: Option<u32>,

  /// if `get_logs_filter_max_limit` is configured but the query would return more logs
  pub get_logs_filter_max_limit: Option<u32>,
}

pub fn convert_config(js_config: ConfluxConfig, temp_dir_path: &Path) -> Configuration {
  let mut conf = Configuration::default();

  // ============= Node Configuration =============
  let node_type = match js_config.node_type.as_deref() {
    Some("archive") => NodeType::Archive,
    Some("light") => NodeType::Light,
    _ => NodeType::Full,
  };
  conf.raw_conf.node_type = Some(node_type);

  conf.raw_conf.block_db_type = js_config.block_db_type.unwrap_or("sqlite".to_string());

  conf.raw_conf.conflux_data_dir = js_config
    .conflux_data_dir
    .unwrap_or(String::from(temp_dir_path.to_str().unwrap()));

  // ============= Chain Configuration =============
  conf.raw_conf.chain_id = Some(js_config.chain_id.unwrap_or(1234));
  conf.raw_conf.evm_chain_id = Some(js_config.evm_chain_id.unwrap_or(1235));
  conf.raw_conf.bootnodes = js_config.bootnodes;

  // ============= Mining Configuration =============
  conf.raw_conf.mining_author = js_config.mining_author;
  conf.raw_conf.stratum_listen_address = js_config
    .stratum_listen_address
    .unwrap_or("127.0.0.1".to_string());
  conf.raw_conf.mining_type = js_config.mining_type;
  conf.raw_conf.stratum_port = js_config.stratum_port.unwrap_or(32525);
  conf.raw_conf.stratum_secret = js_config.stratum_secret;
  conf.raw_conf.pow_problem_window_size = js_config.pow_problem_window_size.unwrap_or(1) as usize;

  // ============= Development Mode Configuration =============
  conf.raw_conf.mode = Some("dev".to_string());
  conf.raw_conf.dev_block_interval_ms = js_config.dev_block_interval_ms.map(|n| n as u64);

  conf.raw_conf.dev_pack_tx_immediately = js_config.dev_pack_tx_immediately;

  if let Some(pk) = js_config.genesis_secrets {
    conf.raw_conf.genesis_secrets = write_secrets_to_file(pk, "genesis_secrets.txt", temp_dir_path);
  }
  if let Some(pk) = js_config.genesis_evm_secrets {
    conf.raw_conf.genesis_evm_secrets =
      write_secrets_to_file(pk, "genesis_evm_secrets.txt", temp_dir_path);
  }

  // ============= Network Configuration =============
  conf.raw_conf.tcp_port = js_config.tcp_port.unwrap_or(32323);
  conf.raw_conf.udp_port = js_config.udp_port.or(Some(32323));

  // ============= JSON-RPC Configuration =============
  conf.raw_conf.public_rpc_apis = match js_config.public_rpc_apis {
    Some(s) => ApiSet::from_str(&s).unwrap_or(ApiSet::All),
    _ => ApiSet::All,
  };

  conf.raw_conf.public_evm_rpc_apis = match js_config.public_evm_rpc_apis {
    Some(s) => ApiSet::from_str(&s).unwrap_or(ApiSet::Evm),
    _ => ApiSet::Evm,
  };

  conf.raw_conf.jsonrpc_ws_port = js_config.jsonrpc_ws_port;
  conf.raw_conf.jsonrpc_http_port = js_config.jsonrpc_http_port;
  conf.raw_conf.jsonrpc_tcp_port = js_config.jsonrpc_tcp_port;
  conf.raw_conf.jsonrpc_http_eth_port = js_config.jsonrpc_http_eth_port;
  conf.raw_conf.jsonrpc_ws_eth_port = js_config.jsonrpc_ws_eth_port;
  conf.raw_conf.jsonrpc_local_tcp_port = js_config.jsonrpc_local_tcp_port;
  conf.raw_conf.jsonrpc_local_http_port = js_config.jsonrpc_local_http_port;
  conf.raw_conf.jsonrpc_local_ws_port = js_config.jsonrpc_local_ws_port;
  conf.raw_conf.jsonrpc_http_keep_alive = js_config.jsonrpc_http_keep_alive.unwrap_or(false);

  // ============= PoS Configuration =============
  conf.raw_conf.dev_pos_private_key_encryption_password = Some(
    js_config
      .dev_pos_private_key_encryption_password
      .unwrap_or("123456".to_string()),
  );
  conf.raw_conf.pos_reference_enable_height =
    js_config.pos_reference_enable_height.unwrap_or(0) as u64;
  conf.raw_conf.pos_config_path = js_config
    .pos_config_path
    .or(Some("./pos_config/pos_config.yaml".to_string()));
  conf.raw_conf.pos_initial_nodes_path = js_config
    .pos_initial_nodes_path
    .unwrap_or("./pos_config/initial_nodes.json".to_string());
  conf.raw_conf.pos_private_key_path = js_config
    .pos_private_key_path
    .unwrap_or("./pos_config/pos_key".to_string());

  // ============= Protocol Upgrade Configuration =============
  conf.raw_conf.default_transition_time =
    Some(js_config.default_transition_time.unwrap_or(1) as u64);
  conf.raw_conf.cip1559_transition_height =
    Some(js_config.cip1559_transition_height.unwrap_or(2) as u64);
  conf.raw_conf.hydra_transition_number =
    Some(js_config.hydra_transition_number.unwrap_or(1) as u64);
  conf.raw_conf.hydra_transition_height =
    Some(js_config.hydra_transition_height.unwrap_or(1) as u64);

  // the transition height of CIP112, default it set in the `Configuration::parse` function, we don't use it here so we set it here
  conf.raw_conf.cip112_transition_height =
    Some(js_config.cip112_transition_height.unwrap_or(1) as u64);
  if CIP112_TRANSITION_HEIGHT.get().is_none() {
    CIP112_TRANSITION_HEIGHT
      .set(js_config.cip112_transition_height.unwrap_or(1) as u64)
      .expect("called once");
  }

  // ============= Logging Configuration =============
  conf.raw_conf.log_conf = js_config.log_conf;
  conf.raw_conf.print_memory_usage_period_s =
    js_config.print_memory_usage_period_s.map(|x| x as u64);

  // ============= Filter and Poll Configuration =============
  conf.raw_conf.poll_lifetime_in_seconds = Some(js_config.poll_lifetime_in_seconds.unwrap_or(60));
  conf.raw_conf.get_logs_filter_max_limit = js_config.get_logs_filter_max_limit.map(|n| n as usize);

  conf
}

fn write_secrets_to_file(
  secrets: Vec<String>,
  filename: &str,
  temp_dir_path: &Path,
) -> Option<std::string::String> {
  let f = File::create(temp_dir_path.join(filename)).unwrap();
  let mut w = BufWriter::new(f);

  for secret in secrets {
    let pk = if let Some(stripped) = secret.strip_prefix("0x") {
      stripped
    } else {
      &secret
    };
    writeln!(w, "{}", pk).unwrap();
  }
  w.flush().unwrap();

  Some(
    temp_dir_path
      .join(filename)
      .into_os_string()
      .into_string()
      .unwrap(),
  )
}
