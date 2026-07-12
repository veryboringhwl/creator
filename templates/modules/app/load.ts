import type { ModuleInstance } from "/hooks/module.ts";
import { createRegistrar } from "/modules/stdlib/mod.ts";

export default async function (mod: ModuleInstance) {
  const registrar = createRegistrar(mod);

  registrar.register("route" /* your route component */);
  registrar.register("settingsSection" /* your settings section */);
}
