import { defineChain } from "cive/utils";

import net from "node:net";
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
      http: ["http://127.0.0.1:12537"],
      webSocket: ["ws://127.0.0.1:12535"],
    },
  },
});

export async function getPortFree(): Promise<number> {
  return new Promise((res, rej) => {
    const srv = net.createServer();
    srv.listen(0, () => {
      const AddressInfo = srv.address();

      if (typeof AddressInfo === "object") {
        if (typeof AddressInfo?.port === "number") {
          return srv.close((err) => res(AddressInfo?.port));
        }
      }
      srv.close((err) => rej("get port error"));
    });
  });
}

export const wait = (ms: number) =>
  new Promise((resolve) => setTimeout(resolve, ms));
