import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { chmod, mkdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import config from "./deno.json" with { type: "json" };

const VERSION = config.version;
const REPO = "veryboringhwl/creator";

const PLATFORM_MAP: Readonly<Record<string, string>> = {
  win32: "windows",
  darwin: "macos",
  linux: "linux",
};

const ARCH_MAP: Readonly<Record<string, string>> = {
  x64: "x86_64",
  arm64: "arm64",
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
  return join(cacheRoot(), `${name}-${VERSION}`);
}

function downloadUrl(name: string): string {
  return `https://github.com/${REPO}/releases/download/${VERSION}/${name}`;
}
//https://github.com/veryboringhwl/v3/releases/latest/download/hooks.tar.gz
async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

async function ensureBinary(): Promise<string> {
  const platform = detectPlatform();
  const name = assetName(platform);
  const bin = binaryPath(name);

  if (await exists(bin)) {
    return bin;
  }

  const cache = cacheRoot();
  await mkdir(cache, { recursive: true });

  const url = downloadUrl(name);
  const tmp = join(cache, `${name}-${VERSION}.${randomUUID()}.tmp`);

  console.error(`Downloading creator v${VERSION} for ${platform.os}-${platform.arch}...`);

  let res: Response;
  try {
    res = await fetch(url);
  } catch (err) {
    console.error(
      `Failed to reach GitHub. Check your network connection.\n` +
        `Error: ${err instanceof Error ? err.message : String(err)}`,
    );
    process.exit(1);
  }

  if (!res.ok) {
    if (res.status === 404) {
      console.error(
        `No binary found for ${platform.os}-${platform.arch}.\n\n` +
          `Ensure a GitHub Release tagged "creator-v${VERSION}" exists ` +
          `with the asset "${name}".\n` +
          `Expected URL: ${url}`,
      );
    } else {
      console.error(`Download failed: HTTP ${res.status} ${res.statusText}`);
    }
    process.exit(1);
  }

  let bytes: Uint8Array;
  try {
    bytes = new Uint8Array(await res.arrayBuffer());
  } catch (err) {
    console.error(
      `Failed to read download stream.\n` +
        `Error: ${err instanceof Error ? err.message : String(err)}`,
    );
    await rm(tmp, { force: true });
    process.exit(1);
  }

  if (bytes.length === 0) {
    console.error("Downloaded binary is empty. The release asset may be corrupt.");
    process.exit(1);
  }

  try {
    await writeFile(tmp, bytes);
  } catch (err) {
    console.error(
      `Failed to write binary to disk.\n` +
        `Path: ${tmp}\n` +
        `Error: ${err instanceof Error ? err.message : String(err)}`,
    );
    process.exit(1);
  }

  if (process.platform !== "win32") {
    try {
      await chmod(tmp, 0o755);
    } catch (err) {
      console.error(
        `Failed to set executable permissions.\n` +
          `Error: ${err instanceof Error ? err.message : String(err)}`,
      );
      process.exit(1);
    }
  }

  try {
    await rename(tmp, bin);
  } catch (err) {
    console.error(
      `Failed to finalize binary installation.\n` +
        `Error: ${err instanceof Error ? err.message : String(err)}`,
    );
    await rm(tmp, { force: true });
    process.exit(1);
  }

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
        console.error(`Creator was killed by signal: ${signal}`);
        process.exit(128 + (typeof signal === "string" ? osSignum(signal) : 0) || 1);
      }
      if (code !== 0 && code !== null) {
        process.exit(code);
      }
      resolve();
    });
  });
}

function osSignum(signal: string): number {
  const map: Record<string, number> = {
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
    SIGTERM: 15,
    SIGSTKFLT: 16,
    SIGCHLD: 17,
    SIGCONT: 18,
    SIGSTOP: 19,
    SIGTSTP: 20,
    SIGTTIN: 21,
    SIGTTOU: 22,
    SIGURG: 23,
    SIGXCPU: 24,
    SIGXFSZ: 25,
    SIGVTALRM: 26,
    SIGPROF: 27,
    SIGWINCH: 28,
    SIGPOLL: 29,
    SIGPWR: 30,
    SIGSYS: 31,
  };
  return map[signal] ?? 0;
}

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
