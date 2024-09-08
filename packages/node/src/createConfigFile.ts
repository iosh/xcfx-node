import fs from "node:fs/promises";
import path from "node:path";
import type { ConfluxConfig } from "./types";

export const CONFIG_FILE_NAME = "config.toml";

export async function createConfigFile({
  genesis_secrets,
  ...config
}: ConfluxConfig) {
  const newConfig: Omit<ConfluxConfig, "genesis_secrets"> & {
    genesis_secrets?: string;
  } = {
    ...config,
    node_type: config.node_type || "full",
    jsonrpc_ws_port: config.jsonrpc_ws_port || 12535,
    jsonrpc_http_port: config.jsonrpc_http_port || 12537,
    public_rpc_apis: config.public_rpc_apis || "all",
    chain_id: config.chain_id || 1234,
    evm_chain_id: config.evm_chain_id || 1235,
    dev_pos_private_key_encryption_password:
      config.dev_pos_private_key_encryption_password || "123456",

    pos_reference_enable_height: config.pos_reference_enable_height || 0,
    dao_vote_transition_number: config.dao_vote_transition_number || 1,
    dao_vote_transition_height: config.dao_vote_transition_height || 1,
    cip43_init_end_number: config.cip43_init_end_number || 1,
    cip71_patch_transition_number: config.cip71_patch_transition_number || 1,

    cip90_transition_height: config.cip90_transition_height || 1,
    cip90_transition_number: config.cip90_transition_number || 1,

    cip105_transition_number: config.cip105_transition_number || 1,
    cip107_transition_number: config.cip107_transition_number || 1,
    cip112_transition_height: config.cip112_transition_height || 1,
    cip118_transition_number: config.cip118_transition_number || 1,
    cip119_transition_number: config.cip119_transition_number || 1,

    next_hardfork_transition_number:
      config.next_hardfork_transition_number || 1,
    next_hardfork_transition_height:
      config.next_hardfork_transition_height || 1,

    cip1559_transition_height: config.cip1559_transition_height || 1,
    cancun_opcodes_transition_number:
      config.cancun_opcodes_transition_number || 1,

    cip113_transition_height: config.cip113_transition_height || 1,
  };
  if (genesis_secrets && Array.isArray(genesis_secrets)) {
    // has the genesis_secrets we need create the genesis_secrets.txt

    const genesisStr = genesis_secrets
      .map((pk) => {
        if (pk.startsWith("0x")) {
          return pk.slice(2);
        }
        return pk;
      })
      .join("\n");

    const genesisPath = path.join(__dirname, "../data/genesis_secrets.txt");
    await fs.writeFile(genesisPath, genesisStr);

    newConfig.genesis_secrets = genesisPath;
  }

  const configStr = Object.entries(newConfig)
    .map(
      ([key, value]) =>
        `${key} = ${typeof value === "string" ? `"${value}"` : value}`,
    )
    .join("\n");
  await fs.writeFile(
    path.join(__dirname, `../data/${CONFIG_FILE_NAME}`),
    configStr,
  );
}
