import { existsSync } from "node:fs";
import { join, resolve } from "node:path";

const args = Deno.args;
const dirs = args.length > 0 ? args : ["modules"];

for (const dir of dirs) {
  const absolute = resolve(dir);
  if (!existsSync(join(absolute, "metadata.json"))) {
    console.warn(`skipping ${dir}: no metadata.json`);
    continue;
  }
  console.log(`Building ${dir}`);
  const exit = await run("creator", [
    "build",
    "-i",
    absolute,
    "-o",
    absolute,
    "-c",
    "classmap.json",
  ]);
  if (exit !== 0) Deno.exit(exit);
}

async function run(command: string, args: string[]): Promise<number> {
  const cmd = new Deno.Command(command, {
    args,
    stdout: "inherit",
    stderr: "inherit",
  });
  const { code } = await cmd.output();
  return code;
}
