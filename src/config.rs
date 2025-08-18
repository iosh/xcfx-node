use crate::error::NodeError;
use cfx_config::Configuration;
use cfx_rpc_builder::RpcModuleSelection;
use cfx_rpc_cfx_types::apis::ApiSet;
use cfxcore::NodeType;
use client::configuration::RawConfiguration;
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
  pub config_file: Option<String>,

  // ============= Node Configuration =============
  /// Set the node type to Full node, Archive node, or Light node.
  ///  Possible values are "full", "archive",or "light".
  /// @default "full"
  pub node_type: Option<String>,

  /// Database type to store block-related data.
  /// Supported: rocksdb, sqlite.
  /// @default sqlite
  pub block_db_type: Option<String>,

  /// Add data directory configuration
  /// The conflux node will use this directory to store data.  If not set, a temporary directory will be used.
  /// @default: temp dir
  pub conflux_data_dir: Option<String>,

  /// Add block db directory configuration
  /// @default: conflux_data_dir + blockchain_db
  pub block_db_dir: Option<String>,

  /// Add netconf directory configuration
  /// @default: conflux_data_dir + net_config
  pub netconf_dir: Option<String>,

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

  /// @default:0
  pub pos_reference_enable_height: Option<i64>,

  /// pos config path
  pub pos_config_path: Option<String>,

  /// pos initial_nodes.json path
  pub pos_initial_nodes_path: Option<String>,

  /// pos pos_key file path
  pub pos_private_key_path: Option<String>,

  // ============= Protocol Upgrade Configuration =============
  /// @default:1
  pub default_transition_time: Option<i64>,

  /// @default:1
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
  /// @default: 600
  pub poll_lifetime_in_seconds: Option<u32>,

  /// if `get_logs_filter_max_limit` is configured but the query would return more logs
  pub get_logs_filter_max_limit: Option<u32>,
}

impl ConfluxConfig {
  pub fn to_configuration(&self, temp_dir_path: &Path) -> Result<Configuration, NodeError> {
    if let Some(config_file) = &self.config_file {
      let conf = Configuration {
        raw_conf: RawConfiguration::from_file(config_file)
          .map_err(|e| NodeError::Configuration(e))?,
      };

      if CIP112_TRANSITION_HEIGHT.get().is_none() {
        CIP112_TRANSITION_HEIGHT
          .set(conf.raw_conf.cip112_transition_height.unwrap_or(u64::MAX))
          .map_err(|_| NodeError::Configuration("CIP112_TRANSITION_HEIGHT already set".into()))?;
      }
      return Ok(conf);
    }

    let mut conf = Configuration::default();

    self.apply_to_raw_config(&mut conf.raw_conf, temp_dir_path)?;

    if CIP112_TRANSITION_HEIGHT.get().is_none() {
      CIP112_TRANSITION_HEIGHT
        .set(conf.raw_conf.cip112_transition_height.unwrap_or(u64::MAX))
        .map_err(|_| NodeError::Configuration("CIP112_TRANSITION_HEIGHT already set".into()))?;
    }

    Ok(conf)
  }

  pub fn apply_to_raw_config(
    &self,
    raw_conf: &mut RawConfiguration,
    temp_dir_path: &Path,
  ) -> Result<(), NodeError> {
    self.apply_node_config(raw_conf);
    self.apply_directory_config(raw_conf, temp_dir_path)?;
    self.apply_chain_config(raw_conf);
    self.apply_mining_config(raw_conf);
    self.apply_dev_config(raw_conf, temp_dir_path)?;
    self.apply_network_config(raw_conf);
    self.apply_rpc_config(raw_conf);
    self.apply_pos_config(raw_conf);
    self.apply_cip_config(raw_conf);
    self.apply_logging_config(raw_conf);
    self.apply_filter_poll_config(raw_conf);
    Ok(())
  }

  fn apply_node_config(&self, raw_conf: &mut RawConfiguration) {
    raw_conf.node_type = match self.node_type.as_deref() {
      Some("archive") => Some(NodeType::Archive),
      Some("light") => Some(NodeType::Light),
      _ => Some(NodeType::Full),
    };

    raw_conf.block_db_type = self
      .block_db_type
      .clone()
      .unwrap_or_else(|| "sqlite".to_string());
  }

  fn apply_directory_config(
    &self,
    raw_conf: &mut RawConfiguration,
    temp_dir_path: &Path,
  ) -> Result<(), NodeError> {
    // Directory configuration

    let data_dir = self
      .conflux_data_dir
      .clone()
      .unwrap_or_else(|| temp_dir_path.to_string_lossy().to_string());

    raw_conf.conflux_data_dir = data_dir.clone();
    raw_conf.block_db_dir = Some(format!("{}/blockchain_db", &data_dir));
    raw_conf.netconf_dir = Some(format!("{}/net_config", &data_dir));
    Ok(())
  }

  fn apply_chain_config(&self, raw_conf: &mut RawConfiguration) {
    // Chain Configuration
    raw_conf.chain_id = Some(self.chain_id.unwrap_or(1234));
    raw_conf.evm_chain_id = Some(self.evm_chain_id.unwrap_or(1235));
    raw_conf.bootnodes = self.bootnodes.clone();
  }

  fn apply_mining_config(&self, raw_conf: &mut RawConfiguration) {
    // Mining Configuration
    raw_conf.mining_author = self.mining_author.clone();
    raw_conf.stratum_listen_address = self
      .stratum_listen_address
      .clone()
      .unwrap_or_else(|| "127.0.0.1".to_string());
    raw_conf.mining_type = self.mining_type.clone();
    raw_conf.stratum_port = self.stratum_port.unwrap_or(32525);
    raw_conf.stratum_secret = self.stratum_secret.clone();
    raw_conf.pow_problem_window_size = self.pow_problem_window_size.unwrap_or(1) as usize;
  }

  fn apply_dev_config(
    &self,
    raw_conf: &mut RawConfiguration,
    temp_dir_path: &Path,
  ) -> Result<(), NodeError> {
    // Development Mode
    raw_conf.mode = Some("dev".to_string());
    raw_conf.dev_block_interval_ms = self.dev_block_interval_ms.map(|n| n as u64);
    raw_conf.dev_pack_tx_immediately = self.dev_pack_tx_immediately;
    // Handle genesis secrets
    if let Some(secrets) = &self.genesis_secrets {
      raw_conf.genesis_secrets =
        Some(self.write_secrets_to_file(secrets.clone(), "genesis_secrets.txt", temp_dir_path)?);
    }
    if let Some(secrets) = &self.genesis_evm_secrets {
      raw_conf.genesis_evm_secrets = Some(self.write_secrets_to_file(
        secrets.clone(),
        "genesis_evm_secrets.txt",
        temp_dir_path,
      )?);
    }

    Ok(())
  }

  fn apply_network_config(&self, raw_conf: &mut RawConfiguration) {
    // Network Configuration
    raw_conf.tcp_port = self.tcp_port.unwrap_or(32323);
    raw_conf.udp_port = self.udp_port.or(Some(32323));
    raw_conf.public_address = self.public_address.clone();
  }

  fn apply_rpc_config(&self, raw_conf: &mut RawConfiguration) {
    // JSON-RPC Configuration
    let default_rpc_apis = ApiSet::from_str("all").unwrap();
    raw_conf.public_rpc_apis = self
      .public_rpc_apis
      .as_ref()
      .map_or(default_rpc_apis.clone(), |s| {
        ApiSet::from_str(s).unwrap_or(default_rpc_apis)
      });

    let default_evm_apis = RpcModuleSelection::from_str("evm,ethdebug").unwrap();
    raw_conf.public_evm_rpc_apis = self
      .public_evm_rpc_apis
      .as_ref()
      .map_or(default_evm_apis.clone(), |s| {
        RpcModuleSelection::from_str(s).unwrap_or(default_evm_apis)
      });

    raw_conf.jsonrpc_ws_port = self.jsonrpc_ws_port;
    raw_conf.jsonrpc_http_port = self.jsonrpc_http_port;
    raw_conf.jsonrpc_tcp_port = self.jsonrpc_tcp_port;
    raw_conf.jsonrpc_http_eth_port = self.jsonrpc_http_eth_port;
    raw_conf.jsonrpc_ws_eth_port = self.jsonrpc_ws_eth_port;
    raw_conf.jsonrpc_local_tcp_port = self.jsonrpc_local_tcp_port;
    raw_conf.jsonrpc_local_http_port = self.jsonrpc_local_http_port;
    raw_conf.jsonrpc_local_ws_port = self.jsonrpc_local_ws_port;
    raw_conf.jsonrpc_http_keep_alive = self.jsonrpc_http_keep_alive.unwrap_or(false);
  }

  fn apply_pos_config(&self, raw_conf: &mut RawConfiguration) {
    raw_conf.dev_pos_private_key_encryption_password = Some(
      self
        .dev_pos_private_key_encryption_password
        .clone()
        .unwrap_or("123456".to_string()),
    );
    raw_conf.pos_reference_enable_height = self.pos_reference_enable_height.unwrap_or(0) as u64;
    raw_conf.pos_config_path = self
      .pos_config_path
      .clone()
      .or(Some("./pos_config/pos_config.yaml".to_string()));
    raw_conf.pos_initial_nodes_path = self
      .pos_initial_nodes_path
      .clone()
      .unwrap_or("./pos_config/initial_nodes.json".to_string());
    raw_conf.pos_private_key_path = self
      .pos_private_key_path
      .clone()
      .unwrap_or("./pos_config/pos_key".to_string());
  }

  fn apply_cip_config(&self, raw_conf: &mut RawConfiguration) {
    raw_conf.default_transition_time = Some(self.default_transition_time.unwrap_or(1) as u64);
    raw_conf.cip1559_transition_height = Some(self.cip1559_transition_height.unwrap_or(1) as u64);
    raw_conf.hydra_transition_number = Some(self.hydra_transition_number.unwrap_or(1) as u64);
    raw_conf.hydra_transition_height = Some(self.hydra_transition_height.unwrap_or(1) as u64);
    raw_conf.cip112_transition_height = Some(self.cip112_transition_height.unwrap_or(1) as u64);
  }

  fn apply_logging_config(&self, raw_conf: &mut RawConfiguration) {
    raw_conf.log_conf = self.log_conf.clone();
    raw_conf.print_memory_usage_period_s = self.print_memory_usage_period_s.map(|x| x as u64);
  }

  fn apply_filter_poll_config(&self, raw_conf: &mut RawConfiguration) {
    // Filter and Poll Configuration
    raw_conf.poll_lifetime_in_seconds = Some(self.poll_lifetime_in_seconds.unwrap_or(600));
    raw_conf.get_logs_filter_max_limit = self.get_logs_filter_max_limit.map(|n| n as usize);
  }

  fn write_secrets_to_file(
    &self,
    secrets: Vec<String>,
    filename: &str,
    dir_path: &Path,
  ) -> Result<String, NodeError> {
    let file_path = dir_path.join(filename);
    let f = File::create(&file_path).map_err(|e| {
      NodeError::Configuration(format!("Failed to create file {}: {}", filename, e))
    })?;
    let mut writer = BufWriter::new(f);

    for secret in secrets {
      let pk = secret.strip_prefix("0x").unwrap_or(&secret);
      writeln!(writer, "{}", pk).map_err(|e| {
        NodeError::Configuration(format!("Failed to write to file {}: {}", filename, e))
      })?;
    }

    writer
      .flush()
      .map_err(|e| NodeError::Configuration(format!("Failed to flush file {}: {}", filename, e)))?;

    Ok(file_path.to_string_lossy().to_string())
  }
}
