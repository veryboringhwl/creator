import { readJSON } from "jsr:@delu/tailor";
import build from "./build-shared.ts";

export const classmapInfos = [
  {
    classmap: await readJSON("classmap.json"),
    version: "1.2.38",
    timestamp: 1675203200,
  },
];

await build(classmapInfos, Deno.args);
