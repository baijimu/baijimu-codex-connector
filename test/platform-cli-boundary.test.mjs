import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("Baijimu CLI exclusively owns platform authentication and Partner API calls", async () => {
  const platformCli = await readFile(join(root, "src", "baijimu_cli.rs"), "utf8");

  for (const forbidden of [
    "load_shared_credential_store",
    "select_local_machine_credential",
    "post_baijimu_json",
    "validateCredential",
    "bearer_auth",
    "machineCredentials",
    "lc_pat_",
    "llm-credential",
    "show-secret",
    "workspace-profiles",
  ]) {
    assert.doesNotMatch(platformCli, new RegExp(forbidden));
  }

  assert.match(platformCli, /\["auth", "status"\]/);
  assert.doesNotMatch(
    platformCli,
    /reqwest|bearer_auth|fs::read|fs::read_to_string|auth\.json/,
  );
});
