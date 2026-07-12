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
  const id = `/Delusoire/${basename(absolute)}@0.0.0-dev`;
  console.log(`Enabling ${id}`);
  await run("spicetify", ["pkg", "delete", id]);
  await run("spicetify", ["pkg", "install", id, absolute]);
  await run("spicetify", ["pkg", "enable", id]);
}

function basename(path: string): string {
  return (
    path
      .replace(/[\\/]+$/, "")
      .split(/[\\/]/)
      .pop() ?? path
  );
}

async function run(command: string, args: string[]): Promise<void> {
  const cmd = new Deno.Command(command, { args, stdout: "inherit", stderr: "inherit" });
  const result = await cmd.output();
  if (!result.success) Deno.exit(result.code);
}
