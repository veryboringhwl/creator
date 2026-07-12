import { existsSync } from "node:fs";
import { resolve } from "node:path";

const args = Deno.args;
const dirs = args.length > 0 ? args : ["modules"];
const absDirs = dirs.map((d) => resolve(d)).filter((d) => existsSync(`${d}/metadata.json`));

if (absDirs.length === 0) {
  console.error("no modules with metadata.json found in:", dirs);
  Deno.exit(1);
}

const url = await resolveClassmapUrl();
const exit = await run("creator", [
  "release",
  ...absDirs.flatMap((d) => ["-i", d]),
  "--classmap-url",
  url,
  "--output-dir",
  "dist",
]);
Deno.exit(exit);

async function resolveClassmapUrl(): Promise<string> {
  const fromEnv = Deno.env.get("CREATOR_CLASSMAP_URL");
  if (fromEnv) return fromEnv.trim();
  try {
    const text = await Deno.readTextFile("classmap.url");
    const trimmed = text.trim();
    if (trimmed) return trimmed;
  } catch (err) {
    if (!(err instanceof Deno.errors.NotFound)) throw err;
  }
  console.error("No classmap URL. Set CREATOR_CLASSMAP_URL or create classmap.url.");
  Deno.exit(1);
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
