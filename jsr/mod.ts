import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, mkdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import config from "./deno.json" with { type: "json" };

const VERSION = config.version;

const PLATFORM_MAP: Readonly<Record<string, string>> = {
  win32: "windows",
  darwin: "macos",
  linux: "linux"
};

const ARCH_MAP: Readonly<Record<string, string>> = {
  x64: "x86_64",
  arm64: "arm64"
};

function detectPlatform(): { os: string; arch: string } {
  const os = PLATFORM_MAP[process.platform];
  const arch = ARCH_MAP[process.arch];

  if (!os) {
    console.error(`Unsupported operating system: ${process.platform}`);
    console.error("Supported: win32 (Windows), darwin (macOS), linux");
    process.exit(1);
  }

  if (!arch) {
    console.error(`Unsupported architecture: ${process.arch}`);
    console.error("Supported: x64 (x86_64), arm64 (aarch64)");
    process.exit(1);
  }

  return { os, arch };
}

function assetName(platform: { os: string; arch: string }): string {
  const ext = process.platform === "win32" ? ".exe" : "";
  return `creator-${platform.os}-${platform.arch}${ext}`;
}

function cacheRoot(): string {
  const home = homedir();

  if (process.platform === "win32") {
    const localAppData = process.env.LOCALAPPDATA;
    if (localAppData) return join(localAppData, "Spicetify", "creator");
  }

  if (process.platform === "darwin") {
    return join(home, "Library", "Caches", "Spicetify", "creator");
  }

  const xdg = process.env.XDG_CACHE_HOME;
  return join(xdg ?? join(home, ".cache"), "Spicetify", "creator");
}

function binaryPath(name: string): string {
  const dotExe = ".exe";
  if (name.endsWith(dotExe)) {
    const base = name.slice(0, -dotExe.length);
    return join(cacheRoot(), `${base}-${VERSION}${dotExe}`);
  }
  return join(cacheRoot(), `${name}-${VERSION}`);
}

function sha256Path(name: string): string {
  return `${binaryPath(name)}.sha256`;
}

async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

function abort(message: string): never {
  console.error(message);
  process.exit(1);
}

async function ensureBinary(): Promise<string> {
  const platform = detectPlatform();
  const name = assetName(platform);
  const bin = binaryPath(name);

  if ((await exists(bin)) && (await exists(sha256Path(name)))) {
    return bin;
  }

  const cache = cacheRoot();
  await mkdir(cache, { recursive: true });

  const baseUrl = `https://github.com/veryboringhwl/creator/releases/download/v${VERSION}`;
  const assetUrl = `${baseUrl}/${name}`;
  const sha256Url = `${baseUrl}/${name}.sha256`;

  console.error(`Downloading creator v${VERSION} for ${platform.os}-${platform.arch}...`);

  const [binRes, shaRes] = await Promise.all([fetch(assetUrl), fetch(sha256Url)]);
  if (!binRes.ok) {
    if (binRes.status === 404) {
      abort(
        `No binary found for ${platform.os}-${platform.arch}.\n\n` +
          `Ensure a GitHub Release tagged "creator-v${VERSION}" exists ` +
          `with the assets "${name}" and "${name}.sha256".\n` +
          `Expected URL: ${assetUrl}`
      );
    }
    abort(`Download failed: HTTP ${binRes.status} ${binRes.statusText}`);
  }
  if (!shaRes.ok) {
    abort(`sha256 manifest missing: HTTP ${shaRes.status} ${shaRes.statusText}`);
  }

  const expectedSha = (await shaRes.text()).trim().split(/\s+/)[0];
  if (!/^[0-9a-f]{64}$/.test(expectedSha)) {
    abort(`sha256 manifest is malformed: ${expectedSha}`);
  }

  let bytes: Uint8Array;
  try {
    bytes = new Uint8Array(await binRes.arrayBuffer());
  } catch (err) {
    abort(
      `Failed to read download stream.\n` +
        `Error: ${err instanceof Error ? err.message : String(err)}`
    );
  }
  if (bytes.length === 0) {
    abort("Downloaded binary is empty. The release asset may be corrupt.");
  }

  const actualSha = createHash("sha256").update(bytes).digest("hex");
  if (actualSha !== expectedSha) {
    abort(
      `sha256 mismatch.\n` +
        `  expected: ${expectedSha}\n` +
        `  actual:   ${actualSha}\n` +
        `Refusing to execute an unverified binary. Delete the partial download ` +
        `in ${cache} and retry.`
    );
  }

  const tmp = join(cache, `${name}-${VERSION}.${crypto.randomUUID()}.tmp`);
  try {
    await writeFile(tmp, bytes);
  } catch (err) {
    abort(
      `Failed to write binary to disk.\n` +
        `Path: ${tmp}\n` +
        `Error: ${err instanceof Error ? err.message : String(err)}`
    );
  }

  if (process.platform !== "win32") {
    try {
      await chmod(tmp, 0o755);
    } catch (err) {
      await rm(tmp, { force: true });
      abort(
        `Failed to set executable permissions.\n` +
          `Error: ${err instanceof Error ? err.message : String(err)}`
      );
    }
  }

  try {
    await rename(tmp, bin);
  } catch (err) {
    await rm(tmp, { force: true });
    abort(
      `Failed to finalize binary installation.\n` +
        `Error: ${err instanceof Error ? err.message : String(err)}`
    );
  }

  await writeFile(sha256Path(name), expectedSha + "\n");

  return bin;
}

function run(bin: string): Promise<void> {
  return new Promise<void>((resolve) => {
    const args = process.argv.slice(2);
    const child = spawn(bin, args, { stdio: "inherit" });

    child.on("error", (err) => {
      console.error(`Failed to spawn creator binary.\nError: ${err.message}`);
      process.exit(1);
    });

    child.on("close", (code, signal) => {
      if (signal) {
        const numeric = osSignals[signal];
        console.error(`Creator was killed by signal: ${signal}`);
        process.exit(128 + (numeric ?? 0));
      }
      if (code !== 0 && code !== null) {
        process.exit(code);
      }
      resolve();
    });
  });
}

const osSignals: Readonly<Record<string, number>> = {
  SIGHUP: 1,
  SIGINT: 2,
  SIGQUIT: 3,
  SIGILL: 4,
  SIGTRAP: 5,
  SIGABRT: 6,
  SIGBUS: 7,
  SIGFPE: 8,
  SIGKILL: 9,
  SIGUSR1: 10,
  SIGSEGV: 11,
  SIGUSR2: 12,
  SIGPIPE: 13,
  SIGALRM: 14,
  SIGTERM: 15
};

function isMain(): boolean {
  if (typeof (import.meta as { main?: boolean }).main === "boolean") {
    return (import.meta as { main?: boolean }).main as boolean;
  }
  try {
    return process.argv[1] === fileURLToPath(import.meta.url);
  } catch {
    return false;
  }
}

if (isMain()) {
  const bin = await ensureBinary();
  await run(bin);
}
