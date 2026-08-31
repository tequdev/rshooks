// Wires rshooks-build's example outputs into the place
// `@transia/hooks-toolkit`'s `readHookBinaryHexFromNS(name, 'wasm')` reads
// from: `${process.cwd()}/build/<name>.wasm` (see
// node_modules/@transia/hooks-toolkit/dist/npm/src/utils.js). Copying
// keeps every test's Hook-building code on the toolkit's own file-reading
// helper instead of bypassing it.
//
// Run before `vitest run` (wired as the `pretest` script). Requires
// `examples/*/out/*.wasm` to already exist - run `mise run build-examples`
// (or the CI `build-hooks` job) first.
import { copyFileSync, existsSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const e2eRoot = dirname(dirname(fileURLToPath(import.meta.url)))
const repoRoot = dirname(e2eRoot)
const buildDir = join(e2eRoot, 'build')

// example directory name (numbered - suggested reading order, see
// examples/README.md) -> [chain-position artifact basename produced by
// rshooks-build under out/current/, destination wasm basename]. Every
// single-hook example's `#[hook(0, ..)]` entry fn is named `main`, so its
// one artifact is always `0.main` - except `19_param-signature`, whose
// entry fn is named `increment` (the Hook Parameter Signature Interface
// draft's own worked example, docs/PARAM_SIGNATURE_DESIGN.md); the
// consolidated `80_governance` chain produces both `0.govern` and
// `1.reward`.
const examples = {
  '01_accept-all': [['0.main', 'accept_all']],
  '02_state-counter': [['0.main', 'state_counter']],
  '03_hook-params': [['0.main', 'hook_params']],
  '04_errors': [['0.main', 'errors']],
  '05_firewall': [['0.main', 'firewall']],
  '06_guard-patterns': [['0.main', 'guard_patterns']],
  '07_xfl-math': [['0.main', 'xfl_math']],
  '08_slot-ledger': [['0.main', 'slot_ledger']],
  '09_state-foreign': [['0.main', 'state_foreign']],
  '10_emit-txn': [['0.main', 'emit_txn']],
  '12_typed-data': [['0.main', 'typed_data']],
  '13_keylets': [['0.main', 'keylets']],
  '14_account-id-macro': [['0.main', 'account_id_macro']],
  '15_slot-objects': [['0.main', 'slot_objects']],
  '19_param-signature': [['0.increment', 'param_signature']],
  '20_state-interface': [['0.main', 'state_interface']],
  '80_governance': [
    ['0.govern', 'govern'],
    ['1.reward', 'reward'],
  ],
}

mkdirSync(buildDir, { recursive: true })

for (const [exampleDir, artifacts] of Object.entries(examples)) {
  for (const [artifact, wasmName] of artifacts) {
    const src = join(repoRoot, 'examples', exampleDir, 'out', 'current', `${artifact}.wasm`)
    if (!existsSync(src)) {
      console.error(
        `error: ${src} not found. Build the examples first: mise run build-examples`,
      )
      process.exit(1)
    }
    const dest = join(buildDir, `${wasmName}.wasm`)
    copyFileSync(src, dest)
    console.log(`copied ${src} -> ${dest}`)
  }
}
