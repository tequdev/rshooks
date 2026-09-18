// Wires rshooks-build's example outputs into the place
// `@xahau/hooks-toolkit`'s `readHookBinaryHexFromNS(name, 'wasm')` reads
// from: `${process.cwd()}/build/<name>.wasm` (see
// node_modules/@xahau/hooks-toolkit/dist/npm/src/utils.js). Copying
// keeps every test's Hook-building code on the toolkit's own file-reading
// helper instead of bypassing it.
//
// Run before `vitest run` (wired as the `pretest` script). Requires
// `examples/*/out/*.wasm` to already exist - run `mise run build-examples`
// (or the CI `build-hooks` job) first.
import { copyFileSync, existsSync, mkdirSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const e2eRoot = dirname(dirname(fileURLToPath(import.meta.url)))
const repoRoot = dirname(e2eRoot)
const buildDir = join(e2eRoot, 'build')

// e2e-covered example directories (numbered - suggested reading order, see
// examples/README.md). Destination wasm basename and source artifact
// basename are derived below, not spelled out here.
const exampleDirs = [
  '01_accept-all',
  '02_state-counter',
  '03_hook-params',
  '04_errors',
  '05_firewall',
  '06_guard-patterns',
  '07_xfl-math',
  '08_slot-ledger',
  '09_state-foreign',
  '10_emit-txn',
  '12_typed-data',
  '13_keylets',
  '14_account-id-macro',
  '15_slot-objects',
  '19_param-signature',
  '20_state-interface',
]

// The consolidated multi-hook chain: two artifacts in one `out/current/`,
// not derivable from "the sole `*.wasm` in the directory".
const GOVERNANCE_DIR = '80_governance'
const governanceArtifacts = [
  ['0.govern', 'govern'],
  ['1.reward', 'reward'],
]

function wasmNameFor(exampleDir) {
  return exampleDir.replace(/^\d+_/, '').replace(/-/g, '_')
}

function copyArtifact(exampleDir, artifact, wasmName) {
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

mkdirSync(buildDir, { recursive: true })

for (const exampleDir of exampleDirs) {
  const currentDir = join(repoRoot, 'examples', exampleDir, 'out', 'current')
  const wasmFiles = existsSync(currentDir)
    ? readdirSync(currentDir).filter((f) => f.endsWith('.wasm'))
    : []
  if (wasmFiles.length !== 1) {
    console.error(
      `error: expected exactly one *.wasm in ${currentDir}, found ${wasmFiles.length}. ` +
        'Build the examples first: mise run build-examples',
    )
    process.exit(1)
  }
  const artifact = wasmFiles[0].replace(/\.wasm$/, '')
  copyArtifact(exampleDir, artifact, wasmNameFor(exampleDir))
}

for (const [artifact, wasmName] of governanceArtifacts) {
  copyArtifact(GOVERNANCE_DIR, artifact, wasmName)
}
