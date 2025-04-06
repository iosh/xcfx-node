import net from "node:net";
import { privateKeyToAccount } from "cive/accounts";
import { defineChain } from "cive/utils";
import path from "node:path";
import fs from "node:fs/promises"
import os from "node:os";

// Default test data directory in system temp folder
export const TEST_TEMP_DATA_DIR = process.env.TEST_TEMP_DIR
  ? process.env.TEST_TEMP_DIR
  : path.join(os.tmpdir(), "xcfx-test-data");

export const sleep = (ms: number) =>
  new Promise((resolve) => setTimeout(resolve, ms));

export async function getPort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.listen(0, () => {
      const AddressInfo = srv.address();

      if (typeof AddressInfo === "object") {
        if (typeof AddressInfo?.port === "number") {
          return srv.close((err) => resolve(AddressInfo?.port));
        }
      }
      srv.close((err) => reject("get port error"));
    });
  });
}

/**
 * Get three free ports
 * @returns Promise<[number, number, number]>
 */
export async function getFreePorts() {
  return Promise.all(Array.from({ length: 3 }).map(() => getPort()));
}

export const wait = (ms: number) =>
  new Promise((resolve) => setTimeout(resolve, ms));

export const TEST_NETWORK_ID = 1111;
export const TEST_MINING_ADDRESS = "0x1b13CC31fC4Ceca3b72e3cc6048E7fabaefB3AC3";
export const TEST_MINING_ADDRESS_PK =
  "0xe590669d866ccd3baf9ec816e3b9f50a964b678e8752386c0d81c39d7e9c6930";

export const MINING_ACCOUNT = privateKeyToAccount(TEST_MINING_ADDRESS_PK, {
  networkId: TEST_NETWORK_ID,
});

export const TEST_PRIVATE_KEYS = [
  "5880f9c6c419f4369b53327bae24e62d805ab0ff825acf7e7945d1f9a0a17bd0",
  "346328c0b7e2abf88fd9c5126192a85eb18b5309c3bd3cc7b61c8b403d1b0e68",
  "bdc6d1c95aaaac8db4eaa5d3d097ba556cf2871149f31e695458d7813543c258",
  "8d108e2aff19b1134c7cc29fbe4a721e599ef21d7d183f64dcd5e78e3a12e5e7",
  "e9cfd4d1d29f7a67c970c1d5c145e958061cd54d05a83e29c7e39b7be894c9c6",
];

export const localChain = defineChain({
  name: "local",
  id: TEST_NETWORK_ID,
  nativeCurrency: {
    decimals: 18,
    name: "CFX",
    symbol: "CFX",
  },
  rpcUrls: {
    default: {
      http: ["http://127.0.0.1:12537"],
      webSocket: ["ws://127.0.0.1:12535"],
    },
  },
});

// biome-ignore format: minimal abi
export const InternalContractsABI = {adminControl:[{inputs:[{internalType:"address",name:"contractAddr",type:"address"},],name:"getAdmin",outputs:[{internalType:"address",name:"",type:"address"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},{internalType:"address",name:"newAdmin",type:"address"},],name:"setAdmin",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},],name:"destroy",outputs:[],stateMutability:"nonpayable",type:"function",},],sponsorWhitelistControl:[{inputs:[{internalType:"address[]",name:"",type:"address[]"}],name:"addPrivilege",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},{internalType:"address[]",name:"addresses",type:"address[]"},],name:"addPrivilegeByAdmin",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},],name:"getSponsorForCollateral",outputs:[{internalType:"address",name:"",type:"address"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},],name:"getSponsorForGas",outputs:[{internalType:"address",name:"",type:"address"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},],name:"getSponsoredBalanceForCollateral",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},],name:"getSponsoredBalanceForGas",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},],name:"getSponsoredGasFeeUpperBound",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},],name:"isAllWhitelisted",outputs:[{internalType:"bool",name:"",type:"bool"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},{internalType:"address",name:"user",type:"address"},],name:"isWhitelisted",outputs:[{internalType:"bool",name:"",type:"bool"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address[]",name:"",type:"address[]"}],name:"removePrivilege",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},{internalType:"address[]",name:"addresses",type:"address[]"},],name:"removePrivilegeByAdmin",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},],name:"setSponsorForCollateral",outputs:[],stateMutability:"payable",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},{internalType:"uint256",name:"upperBound",type:"uint256"},],name:"setSponsorForGas",outputs:[],stateMutability:"payable",type:"function",},{inputs:[{internalType:"address",name:"contractAddr",type:"address"},],name:"getAvailableStoragePoints",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},],staking:[{inputs:[{internalType:"address",name:"user",type:"address"}],name:"getStakingBalance",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"user",type:"address"},{internalType:"uint256",name:"blockNumber",type:"uint256"},],name:"getLockedStakingBalance",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"user",type:"address"},{internalType:"uint256",name:"blockNumber",type:"uint256"},],name:"getVotePower",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"uint256",name:"amount",type:"uint256"}],name:"deposit",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"uint256",name:"amount",type:"uint256"}],name:"withdraw",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"uint256",name:"amount",type:"uint256"},{internalType:"uint256",name:"unlockBlockNumber",type:"uint256"},],name:"voteLock",outputs:[],stateMutability:"nonpayable",type:"function",},],confluxContext:[{inputs:[],name:"epochNumber",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},{inputs:[],name:"posHeight",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},{inputs:[],name:"finalizedEpochNumber",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},],poSRegister:[{anonymous:false,inputs:[{indexed:true,internalType:"bytes32",name:"identifier",type:"bytes32",},{indexed:false,internalType:"uint64",name:"votePower",type:"uint64",},],name:"IncreaseStake",type:"event",},{anonymous:false,inputs:[{indexed:true,internalType:"bytes32",name:"identifier",type:"bytes32",},{indexed:false,internalType:"bytes",name:"blsPubKey",type:"bytes",},{indexed:false,internalType:"bytes",name:"vrfPubKey",type:"bytes",},],name:"Register",type:"event",},{anonymous:false,inputs:[{indexed:true,internalType:"bytes32",name:"identifier",type:"bytes32",},{indexed:false,internalType:"uint64",name:"votePower",type:"uint64",},],name:"Retire",type:"event",},{inputs:[{internalType:"bytes32",name:"indentifier",type:"bytes32"},{internalType:"uint64",name:"votePower",type:"uint64"},{internalType:"bytes",name:"blsPubKey",type:"bytes"},{internalType:"bytes",name:"vrfPubKey",type:"bytes"},{internalType:"bytes[2]",name:"blsPubKeyProof",type:"bytes[2]"},],name:"register",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"uint64",name:"votePower",type:"uint64"}],name:"increaseStake",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"uint64",name:"votePower",type:"uint64"}],name:"retire",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"bytes32",name:"identifier",type:"bytes32"},],name:"getVotes",outputs:[{internalType:"uint256",name:"",type:"uint256"},{internalType:"uint256",name:"",type:"uint256"},],stateMutability:"view",type:"function",},{inputs:[{internalType:"bytes32",name:"identifier",type:"bytes32"},],name:"identifierToAddress",outputs:[{internalType:"address",name:"",type:"address"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"addr",type:"address"}],name:"addressToIdentifier",outputs:[{internalType:"bytes32",name:"",type:"bytes32"}],stateMutability:"view",type:"function",},],crossSpaceCall:[{anonymous:false,inputs:[{indexed:true,internalType:"bytes20",name:"sender",type:"bytes20",},{indexed:true,internalType:"bytes20",name:"receiver",type:"bytes20",},{indexed:false,internalType:"uint256",name:"value",type:"uint256",},{indexed:false,internalType:"uint256",name:"nonce",type:"uint256",},{indexed:false,internalType:"bytes",name:"data",type:"bytes"},],name:"Call",type:"event",},{anonymous:false,inputs:[{indexed:true,internalType:"bytes20",name:"sender",type:"bytes20",},{indexed:true,internalType:"bytes20",name:"contract_address",type:"bytes20",},{indexed:false,internalType:"uint256",name:"value",type:"uint256",},{indexed:false,internalType:"uint256",name:"nonce",type:"uint256",},{indexed:false,internalType:"bytes",name:"init",type:"bytes"},],name:"Create",type:"event",},{anonymous:false,inputs:[{indexed:false,internalType:"bool",name:"success",type:"bool"},],name:"Outcome",type:"event",},{anonymous:false,inputs:[{indexed:true,internalType:"bytes20",name:"sender",type:"bytes20",},{indexed:true,internalType:"address",name:"receiver",type:"address",},{indexed:false,internalType:"uint256",name:"value",type:"uint256",},{indexed:false,internalType:"uint256",name:"nonce",type:"uint256",},],name:"Withdraw",type:"event",},{inputs:[{internalType:"bytes",name:"init",type:"bytes"}],name:"createEVM",outputs:[{internalType:"bytes20",name:"",type:"bytes20"}],stateMutability:"payable",type:"function",},{inputs:[{internalType:"bytes20",name:"to",type:"bytes20"}],name:"transferEVM",outputs:[{internalType:"bytes",name:"output",type:"bytes"}],stateMutability:"payable",type:"function",},{inputs:[{internalType:"bytes20",name:"to",type:"bytes20"},{internalType:"bytes",name:"data",type:"bytes"},],name:"callEVM",outputs:[{internalType:"bytes",name:"output",type:"bytes"}],stateMutability:"payable",type:"function",},{inputs:[{internalType:"bytes20",name:"to",type:"bytes20"},{internalType:"bytes",name:"data",type:"bytes"},],name:"staticCallEVM",outputs:[{internalType:"bytes",name:"output",type:"bytes"}],stateMutability:"view",type:"function",},{inputs:[],name:"deployEip1820",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"uint256",name:"value",type:"uint256"}],name:"withdrawFromMapped",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[{internalType:"address",name:"addr",type:"address"}],name:"mappedBalance",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"addr",type:"address"}],name:"mappedNonce",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},],paramsControl:[{anonymous:false,inputs:[{indexed:true,internalType:"uint64",name:"vote_round",type:"uint64",},{indexed:true,internalType:"address",name:"addr",type:"address",},{indexed:true,internalType:"uint16",name:"topic_index",type:"uint16",},{indexed:false,internalType:"uint256[3]",name:"votes",type:"uint256[3]",},],name:"CastVote",type:"event",},{anonymous:false,inputs:[{indexed:true,internalType:"uint64",name:"vote_round",type:"uint64",},{indexed:true,internalType:"address",name:"addr",type:"address",},{indexed:true,internalType:"uint16",name:"topic_index",type:"uint16",},{indexed:false,internalType:"uint256[3]",name:"votes",type:"uint256[3]",},],name:"RevokeVote",type:"event",},{inputs:[{internalType:"uint64",name:"vote_round",type:"uint64"},{components:[{internalType:"uint16",name:"topic_index",type:"uint16"},{internalType:"uint256[3]",name:"votes",type:"uint256[3]"},],internalType:"struct ParamsControl.Vote[]",name:"vote_data",type:"tuple[]",},],name:"castVote",outputs:[],stateMutability:"nonpayable",type:"function",},{inputs:[],name:"currentRound",outputs:[{internalType:"uint64",name:"",type:"uint64"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"uint64",name:"",type:"uint64"}],name:"posStakeForVotes",outputs:[{internalType:"uint256",name:"",type:"uint256"}],stateMutability:"view",type:"function",},{inputs:[{internalType:"address",name:"addr",type:"address"}],name:"readVote",outputs:[{components:[{internalType:"uint16",name:"topic_index",type:"uint16"},{internalType:"uint256[3]",name:"votes",type:"uint256[3]"},],internalType:"struct ParamsControl.Vote[]",name:"",type:"tuple[]",},],stateMutability:"view",type:"function",},{inputs:[{internalType:"uint64",name:"vote_round",type:"uint64"}],name:"totalVotes",outputs:[{components:[{internalType:"uint16",name:"topic_index",type:"uint16"},{internalType:"uint256[3]",name:"votes",type:"uint256[3]"},],internalType:"struct ParamsControl.Vote[]",name:"",type:"tuple[]",},],stateMutability:"view",type:"function",},],} as const;

/**
 * Retry delete file or directory with exponential backoff
 * @param filePath - Path to file or directory to delete
 * @param isDirectory - Whether the path is a directory
 * @param maxAttempts - Maximum number of retry attempts
 * @returns Promise<boolean> - Whether deletion was successful
 */
export const retryDelete = async (
  filePath: string,
  maxAttempts = 5,
) => {
  await fs.rm(filePath, {
    recursive:true,
    maxRetries: maxAttempts,
    force:true,
    retryDelay: 300
    });
};
