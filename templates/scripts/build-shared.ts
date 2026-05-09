import { Builder, readJSON, Transpiler } from "jsr:@delu/tailor";

import { ensureDir } from "jsr:@std/fs/ensure-dir";

import path from "node:path";

type ClassmapInfo = {
  classmap: any;
  version: string;
  timestamp: number | string;
};
export default async function (classmapInfos: ClassmapInfo[], inputDirs: string[]) {
  for (const inputDir of inputDirs) {
    const metadata = await readJSON<any>(path.join(inputDir, "metadata.json"));

    for (const { classmap, version: spVersion, timestamp: cmTimestamp } of classmapInfos) {
      const m = { ...metadata };
      m.version = `${metadata.version}+sp-${spVersion}-cm-${cmTimestamp}`;
      const fingerprint = `${m.authors[0]}.${m.name}@v${m.version}`;
      const outputDir = path.join("dist", fingerprint);

      await ensureDir(outputDir);

      const transpiler = new Transpiler(classmap);
      const builder = new Builder(transpiler, { metadata, inputDir, outputDir, copyUnknown: true });

      try {
        await builder.build();
        await Deno.writeTextFile(path.join(outputDir, "metadata.json"), JSON.stringify(m));
      } catch (err) {
        await Deno.remove(outputDir, { recursive: true });
        console.warn(`Build for ${fingerprint} failed with error: ${err}`);
      }
    }
  }
}
