use cfxcore::NodeType;
use client::{common::Configuration, rpc::rpc_apis::ApiSet};
use napi_derive::napi;
use std::{
  fs::File,
  io::{BufWriter, Write},
  path::Path,
  str::FromStr,
};

#[napi(object)]
#[derive(Debug)]
pub struct ConfluxConfig {
  /// Set the node type to Full node, Archive node, or Light node.
  ///  Possible values are "full", "archive",or"light".
  /// @default "full"
  pub node_type: Option<String>,
  //   /// `dev` mode is for users to run a single node that automatically
  //   ///  @default "dev"
  //   pub mode: Option<String>,
  /// If it's not set, blocks will only be generated after receiving a transaction.Otherwise,
  /// the blocks are automatically generated every
  pub dev_block_interval_ms: Option<i64>,
  /// mining_author is the address to receive mining rewards.
  pub mining_author: Option<String>,
  /// Listen address for stratum
  /// @default "127.0.0.1"
  pub stratum_listen_address: Option<String>,
  /// Port for stratum.
  /// @default 32525
  pub stratum_port: Option<u16>,
  /// log_conf` the path of the log4rs configuration file. The configuration in the file will overwrite the value set by `log_level`.
  pub log_conf: Option<String>,
  /// `log_file` is the path of the log file"
  /// If not set, the log will only be printed to stdout, and not persisted to files.
  pub log_file: Option<String>,
  /// log_level` is the printed log level.
  /// "error" | "warn" | "info" | "debug" | "trace" | "off"
  pub log_level: Option<String>,
  /// The port of the websocket JSON-RPC server(public_rpc_apis is user defined).
  /// if not set, the JSON-RPC server will not be started.
  /// @default null
  pub jsonrpc_ws_port: Option<u16>,

  /// `tcp_port` is the TCP port that the process listens for P2P messages. The default is 32323.
  /// @default 32323
  pub tcp_port: Option<u16>,
  /// `udp_port` is the UDP port used for node discovery.
  /// @default 32323
  pub udp_port: Option<u16>,

  /// Possible Core space names are: all, safe, cfx, pos, debug, pubsub, test, trace, txpool.
  /// `safe` only includes `cfx` and `pubsub`, `txpool`.
  ///  @default "all"
  pub public_rpc_apis: Option<String>,
  /// Possible eSpace names are: eth, ethpubsub, ethdebug.
  ///  @default 'evm'
  pub public_evm_rpc_apis: Option<String>,
  /// The chain ID of the network.(core space)
  /// @default 1234
  pub chain_id: Option<u32>,
  /// The chain ID of the network.( eSpace)
  ///  @default 1235
  pub evm_chain_id: Option<u32>,
  /// The password used to encrypt the private key of the pos_private_key.
  ///  @default "123456"
  pub dev_pos_private_key_encryption_password: Option<String>,
  ///  The private key of the genesis, every account will be receive 10000 CFX
  pub genesis_secrets: Option<Vec<String>>,
  /// If it's `true`, `DEFERRED_STATE_EPOCH_COUNT` blocks are generated after
  /// receiving a new tx through RPC calling to pack and execute this
  /// transaction
  pub dev_pack_tx_immediately: Option<bool>,
  /// @default:1
  pub pos_reference_enable_height: Option<i64>,
  /// @default:2
  pub default_transition_time: Option<i64>,
  /// @default:3
  pub cip1559_transition_height: Option<i64>,
  /// @default: temp dir
  pub conflux_data_dir: Option<String>,
  /// pos config path
  pub pos_config_path: Option<String>,
  /// pos initial_nodes.json path
  pub pos_initial_nodes_path: Option<String>,
  /// pos pos_key file path
  pub pos_private_key_path: Option<String>,
  /// Database type to store block-related data.
  /// Supported: rocksdb, sqlite.
  /// @default sqlite
  pub block_db_type: Option<String>,

  /// bootnodes is a list of nodes that a conflux node trusts, and will be used to sync the blockchain when a node starts.
  pub bootnodes: Option<String>,

  /// Window size for PoW manager
  pub pow_problem_window_size: Option<u32>,
  /// # Secret key for stratum. The value is 64-digit hex string. If not set, the RPC subscription will not check the authorization.
  pub stratum_secret: Option<String>,
  /// `public_address` is the address of this node used
  pub public_address: Option<String>,
  /// `jsonrpc_http_keep_alive` is used to control whether to set KeepAlive for rpc HTTP connections.
  /// @default false
  pub jsonrpc_http_keep_alive: Option<bool>,
  /// `print_memory_usage_period_s` is the period for printing memory usage.
  pub print_memory_usage_period_s: Option<u32>,
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
}

pub fn convert_config(js_config: ConfluxConfig, temp_dir_path: &Path) -> Configuration {
  let mut conf = Configuration::default();

  let node_type = match js_config.node_type.as_deref() {
    Some("archive") => NodeType::Archive,
    Some("light") => NodeType::Light,
    _ => NodeType::Full,
  };
  // node_type
  conf.raw_conf.node_type = Some(node_type);

  // chain
  conf.raw_conf.chain_id = Some(js_config.chain_id.unwrap_or(1234));

  conf.raw_conf.evm_chain_id = Some(js_config.evm_chain_id.unwrap_or(1235));

  if let Some(pk) = js_config.genesis_secrets {
    let f = File::create(temp_dir_path.join("genesis_secrets.txt")).unwrap();
    let mut w = BufWriter::new(f);

    for p in pk {
      writeln!(w, "{}", p).unwrap();
    }
    w.flush().unwrap();

    conf.raw_conf.genesis_secrets = Some(
      temp_dir_path
        .join("genesis_secrets.txt")
        .into_os_string()
        .into_string()
        .unwrap(),
    );
  }

  // mode
  conf.raw_conf.mode = Some("dev".to_string());

  conf.raw_conf.dev_block_interval_ms = match js_config.dev_block_interval_ms {
    Some(n) => Some(n as u64),
    _ => None,
  };

  conf.raw_conf.mining_author = js_config.mining_author;

  conf.raw_conf.public_rpc_apis = match js_config.public_rpc_apis {
    Some(s) => ApiSet::from_str(&s).unwrap_or(ApiSet::All),
    _ => ApiSet::All,
  };

  conf.raw_conf.public_evm_rpc_apis = match js_config.public_evm_rpc_apis {
    Some(s) => ApiSet::from_str(&s).unwrap_or(ApiSet::Evm),
    _ => ApiSet::Evm,
  };

  conf.raw_conf.dev_pos_private_key_encryption_password = Some(
    js_config
      .dev_pos_private_key_encryption_password
      .unwrap_or("123456".to_string()),
  );

  conf.raw_conf.dev_pack_tx_immediately = js_config.dev_pack_tx_immediately;

  conf.raw_conf.pos_reference_enable_height =
    js_config.pos_reference_enable_height.unwrap_or(0) as u64;

  conf.raw_conf.default_transition_time =
    Some(js_config.default_transition_time.unwrap_or(1) as u64);

  conf.raw_conf.cip1559_transition_height =
    Some(js_config.cip1559_transition_height.unwrap_or(2) as u64);

  // confix data dir, default to temp dir
  conf.raw_conf.conflux_data_dir = js_config
    .conflux_data_dir
    .unwrap_or(String::from(temp_dir_path.to_str().unwrap()));
  // pos configs
  conf.raw_conf.pos_config_path = js_config
    .pos_config_path
    .or(Some("./pos_config/pos_config.yaml".to_string()));

  conf.raw_conf.pos_initial_nodes_path = js_config
    .pos_initial_nodes_path
    .unwrap_or("./pos_config/initial_nodes.json".to_string());

  conf.raw_conf.pos_private_key_path = js_config
    .pos_private_key_path
    .unwrap_or("./pos_config/pos_key".to_string());

  // set the block db to sqlite
  conf.raw_conf.block_db_type = js_config.block_db_type.unwrap_or("sqlite".to_string());
  conf.raw_conf.log_conf = js_config.log_conf;

  // network

  conf.raw_conf.stratum_listen_address = js_config
    .stratum_listen_address
    .unwrap_or("127.0.0.1".to_string());

  conf.raw_conf.jsonrpc_ws_port = js_config.jsonrpc_ws_port;
  conf.raw_conf.jsonrpc_http_port = js_config.jsonrpc_http_port;

  conf.raw_conf.stratum_port = js_config.stratum_port.unwrap_or(32525);

  conf.raw_conf.tcp_port = js_config.tcp_port.unwrap_or(32323);

  conf.raw_conf.udp_port = js_config.udp_port.or(Some(32323));

  // unnecessary config
  conf.raw_conf.bootnodes = js_config.bootnodes;

  conf.raw_conf.pow_problem_window_size = js_config.pow_problem_window_size.unwrap_or(1) as usize;

  conf.raw_conf.stratum_secret = js_config.stratum_secret;

  conf.raw_conf.jsonrpc_http_keep_alive = js_config.jsonrpc_http_keep_alive.unwrap_or(false);

  conf.raw_conf.jsonrpc_tcp_port = js_config.jsonrpc_tcp_port;
  conf.raw_conf.jsonrpc_http_eth_port = js_config.jsonrpc_http_eth_port;
  conf.raw_conf.jsonrpc_ws_eth_port = js_config.jsonrpc_ws_eth_port;

  conf.raw_conf.jsonrpc_local_tcp_port = js_config.jsonrpc_local_tcp_port;
  conf.raw_conf.jsonrpc_local_http_port = js_config.jsonrpc_local_http_port;
  conf.raw_conf.jsonrpc_local_ws_port = js_config.jsonrpc_local_ws_port;

  conf.raw_conf.print_memory_usage_period_s =
    js_config.print_memory_usage_period_s.map(|x| x as u64);

  conf
}
