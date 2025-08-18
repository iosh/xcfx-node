import { createPublicClient, http } from "cive";
import { encodeFunctionData, hexAddressToBase32 } from "cive/utils";
import { describe, expect, test } from "vitest";
import { createServer } from "../index";
import {
  getFreePorts,
  InternalContractsABI,
  TEST_NETWORK_ID,
  TEST_PRIVATE_KEYS,
} from "./help";

/**
 * Test internal contracts functionality
 * Shows how to:
 * 1. Access and interact with Conflux internal contracts
 * 2. Call various internal contract methods
 * 3. Verify initial states of internal contracts
 */
describe("Internal Contracts", () => {
  test("should access all internal contracts with correct initial states", async () => {
    // Setup node with minimal configuration
    const [jsonrpcHttpPort, udpAndTcpPort] = await getFreePorts();
    const server = await createServer({
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,
      chainId: TEST_NETWORK_ID,
      devBlockIntervalMs: 100,
      jsonrpcHttpPort: jsonrpcHttpPort,
      genesisSecrets: TEST_PRIVATE_KEYS,
    });
    await server.start();

    // Create RPC client
    const client = createPublicClient({
      transport: http(`http://127.0.0.1:${jsonrpcHttpPort}`),
    });

    // Test AdminControl contract
    const adminControlAddress = hexAddressToBase32({
      hexAddress: "0x0888000000000000000000000000000000000000",
      networkId: TEST_NETWORK_ID,
    });
    const adminControl = await client.call({
      to: adminControlAddress,
      data: encodeFunctionData({
        abi: InternalContractsABI.adminControl,
        functionName: "getAdmin",
        args: [adminControlAddress],
      }),
    });
    expect(adminControl.data).toBe(
      "0x0000000000000000000000000000000000000000000000000000000000000000",
    );

    // Test SponsorWhitelistControl contract
    const sponsorWhitelistControlAddress = hexAddressToBase32({
      hexAddress: "0x0888000000000000000000000000000000000001",
      networkId: TEST_NETWORK_ID,
    });
    const sponsorWhitelistControl = await client.call({
      to: sponsorWhitelistControlAddress,
      data: encodeFunctionData({
        abi: InternalContractsABI.sponsorWhitelistControl,
        functionName: "getSponsorForGas",
        args: [adminControlAddress],
      }),
    });
    expect(sponsorWhitelistControl.data).toBe(
      "0x0000000000000000000000000000000000000000000000000000000000000000",
    );

    // Test Staking contract
    const stakingAddress = hexAddressToBase32({
      hexAddress: "0x0888000000000000000000000000000000000002",
      networkId: TEST_NETWORK_ID,
    });
    const staking = await client.call({
      to: stakingAddress,
      data: encodeFunctionData({
        abi: InternalContractsABI.staking,
        functionName: "getStakingBalance",
        args: [adminControlAddress],
      }),
    });
    expect(staking.data).toBe(
      "0x0000000000000000000000000000000000000000000000000000000000000000",
    );

    // Test ConfluxContext contract
    const confluxContextAddress = hexAddressToBase32({
      hexAddress: "0x0888000000000000000000000000000000000004",
      networkId: TEST_NETWORK_ID,
    });
    const confluxContext = await client.call({
      to: confluxContextAddress,
      data: encodeFunctionData({
        abi: InternalContractsABI.confluxContext,
        functionName: "finalizedEpochNumber",
      }),
    });
    expect(confluxContext.data).toBe(
      "0x0000000000000000000000000000000000000000000000000000000000000000",
    );

    // Test PoSRegister contract
    const poSRegisterAddress = hexAddressToBase32({
      hexAddress: "0x0888000000000000000000000000000000000005",
      networkId: TEST_NETWORK_ID,
    });
    const poSRegister = await client.call({
      to: poSRegisterAddress,
      data: encodeFunctionData({
        abi: InternalContractsABI.poSRegister,
        functionName: "addressToIdentifier",
        args: [adminControlAddress],
      }),
    });
    expect(poSRegister.data).toBe(
      "0x0000000000000000000000000000000000000000000000000000000000000000",
    );

    // Test CrossSpaceCall contract
    const crossSpaceCallAddress = hexAddressToBase32({
      hexAddress: "0x0888000000000000000000000000000000000006",
      networkId: TEST_NETWORK_ID,
    });
    const crossSpaceCall = await client.call({
      to: crossSpaceCallAddress,
      data: encodeFunctionData({
        abi: InternalContractsABI.crossSpaceCall,
        functionName: "mappedNonce",
        args: [adminControlAddress],
      }),
    });
    expect(crossSpaceCall.data).toBe(
      "0x0000000000000000000000000000000000000000000000000000000000000000",
    );

    // Test ParamsControl contract
    const paramsControlAddress = hexAddressToBase32({
      hexAddress: "0x0888000000000000000000000000000000000007",
      networkId: TEST_NETWORK_ID,
    });
    const paramsControl = await client.call({
      to: paramsControlAddress,
      data: encodeFunctionData({
        abi: InternalContractsABI.paramsControl,
        functionName: "currentRound",
      }),
    });
    expect(paramsControl.data).toBeUndefined();

    await server.stop();
  });
});
