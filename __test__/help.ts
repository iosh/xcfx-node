import { privateKeyToAccount } from "cive/accounts";
import { defineChain } from "cive/utils";

export const poolId = Number(process.env.VITEST_POOL_ID ?? 1);

export const jsonrpcHttpPort = 12500 + poolId;
export const jsonrpcWsPort = 12700 + poolId;

export const udpAndTcpPort = 12900 + poolId;

export const localChain = defineChain({
  name: "local",
  id: 1234,
  nativeCurrency: {
    decimals: 18,
    name: "CFX",
    symbol: "CFX",
  },
  rpcUrls: {
    default: {
      http: [`http://127.0.0.1:${jsonrpcHttpPort}`],
      webSocket: [`ws://127.0.0.1:${jsonrpcWsPort}`],
    },
  },
});

export const wait = (ms: number) =>
  new Promise((resolve) => setTimeout(resolve, ms));

export const TEST_NETWORK_ID = 1111;
export const TEST_MINING_ADDRESS = "0x1b13CC31fC4Ceca3b72e3cc6048E7fabaefB3AC3";
export const TEST_MINING_ADDRESS_PK =
  "0xe590669d866ccd3baf9ec816e3b9f50a964b678e8752386c0d81c39d7e9c6930";

export const MINING_ACCOUNT = privateKeyToAccount(TEST_MINING_ADDRESS_PK, {
  networkId: TEST_NETWORK_ID,
});

export const TEST_PK = [
  "5880f9c6c419f4369b53327bae24e62d805ab0ff825acf7e7945d1f9a0a17bd0",
  "346328c0b7e2abf88fd9c5126192a85eb18b5309c3bd3cc7b61c8b403d1b0e68",
  "bdc6d1c95aaaac8db4eaa5d3d097ba556cf2871149f31e695458d7813543c258",
  "8d108e2aff19b1134c7cc29fbe4a721e599ef21d7d183f64dcd5e78e3a12e5e7",
  "e9cfd4d1d29f7a67c970c1d5c145e958061cd54d05a83e29c7e39b7be894c9c6",
];
