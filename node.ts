import { ConfluxNode, type ConfluxConfig } from "./conflux";

process.on("message", (message: { type: string; config?: ConfluxConfig }) => {
  if (!process.send) return;

  const node = new ConfluxNode();

  if (message.type === "start" && message.config) {
    node.startNode(message.config, (err) => {
      if (err) {
        process.send!({ type: "error", error: err.message });
      } else {
        process.send!({ type: "started" });
      }
    });
  } else if (message.type === "stop") {
    node.stopNode((err) => {
      if (err) {
        process.send!({ type: "error", error: err.message });
      } else {
        process.send!({ type: "stopped" });
        process.exit(0);
      }
    });
  }
});

process.on("uncaughtException", (err) => {
  if (process.send) {
    process.send({ type: "error", error: err.message });
  }
  process.exit(1);
});
