import { defineChain } from "cive/utils";

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
