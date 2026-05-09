import type { IndexLoadFn, IndexMixinFn } from "/hooks/module.ts";

export const mixins: IndexMixinFn = async (context) => {
  return await (await import("./mixin.ts")).default(context.module);
};
export const load: IndexLoadFn = async (context) => {
  return await (await import("./load.ts")).default(context.module);
};
