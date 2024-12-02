import { http, createPublicClient } from "cive";
import { encodeFunctionData, hexAddressToBase32 } from "cive/utils";
import { describe, expect, test } from "vitest";
import { createServer } from "../index";
import {
  InternalContractsABI,
  TEST_NETWORK_ID,
  TEST_PK,
  getFreePorts,
} from "./help";

describe("genesis", () => {
  test("default", async () => {
    const [jsonrpcHttpPort, udpAndTcpPort] = await getFreePorts();
    const server = await createServer({
      tcpPort: udpAndTcpPort,
      udpPort: udpAndTcpPort,
      chainId: TEST_NETWORK_ID,
      devBlockIntervalMs: 100,
      jsonrpcHttpPort: jsonrpcHttpPort,
      genesisSecrets: TEST_PK,
    });
    await server.start();
    const client = createPublicClient({
      transport: http(`http://127.0.0.1:${jsonrpcHttpPort}`),
    });

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
    expect(adminControl).toMatchInlineSnapshot(`
      {
        "data": "0x0000000000000000000000000000000000000000000000000000000000000000",
      }
    `);

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

    expect(sponsorWhitelistControl).toMatchInlineSnapshot(`
      {
        "data": "0x0000000000000000000000000000000000000000000000000000000000000000",
      }
    `);

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

    expect(staking).toMatchInlineSnapshot(`
      {
        "data": "0x0000000000000000000000000000000000000000000000000000000000000000",
      }
    `);

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

    expect(confluxContext).toMatchInlineSnapshot(`
      {
        "data": "0x0000000000000000000000000000000000000000000000000000000000000000",
      }
    `);

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

    expect(poSRegister).toMatchInlineSnapshot(`
      {
        "data": "0x0000000000000000000000000000000000000000000000000000000000000000",
      }
    `);

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

    expect(crossSpaceCall).toMatchInlineSnapshot(`
      {
        "data": "0x0000000000000000000000000000000000000000000000000000000000000000",
      }
    `);

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

    expect(paramsControl).toMatchInlineSnapshot(`
      {
        "data": undefined,
      }
    `);
    await server.stop();
  });
});
