import fs from "node:fs";
import path from "node:path";

const rmsyncOptions = {
  force: true,
  recursive: true,
};

export function cleanup(workDir: string) {
  // remove conflux data
  fs.rmSync(path.join(workDir, "blockchain_data"), rmsyncOptions);
  // remove conflux db
  fs.rmSync(path.join(workDir, "db"), rmsyncOptions);
  // remove conflux log
  fs.rmSync(path.join(workDir, "log"), rmsyncOptions);
  // remove conflux pos_db
  fs.rmSync(path.join(workDir, "pos_db"), rmsyncOptions);
}
