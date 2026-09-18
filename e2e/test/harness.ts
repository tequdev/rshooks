// Shared install/teardown for every suite under `e2e/test/`: each one
// deploys one example's wasm to a fresh standalone-node client, exercises
// it, then tears the hook down. `installHook` wires that `beforeAll`/
// `afterAll` pair; `buildHook` is the bare `iHook` builder for suites
// (govern.test.ts) that install more than once per suite.
// `readWorstCaseHook` reads the CI-gated static bound straight from
// `examples/<dir>/metrics.json` instead of a copy that can drift from it.

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  clearAllHooks,
  clearHookState,
  hexNamespace,
  readHookBinaryHexFromNS,
  serverUrl,
  setHooks,
  setupClient,
  teardownClient,
  type XrplIntegrationTestContext,
  type iHook,
} from '@xahau/hooks-toolkit'
import { calculateHookOn } from 'xahau'
import { HookFlags } from 'xahau/dist/npm/models/common/xahau'

export type Wallet = XrplIntegrationTestContext['alice']
export type HookParam = {
  HookParameter: { HookParameterName: string; HookParameterValue: string }
}

// This file lives in `e2e/test/`, mirroring `e2e/scripts/copy-wasm.mjs`'s
// own two-level walk up to the repo root.
const e2eRoot = dirname(dirname(fileURLToPath(import.meta.url)))
export const repoRoot = dirname(e2eRoot)

/** Builds one `iHook` for `setHooks` - the object every suite otherwise assembles by hand. */
export function buildHook(
  wasmName: string,
  namespace: string,
  hookOn: string[],
  hookParameters?: HookParam[],
): iHook {
  return {
    CreateCode: readHookBinaryHexFromNS(wasmName, 'wasm'),
    Flags: HookFlags.hsfOverride,
    HookOn: calculateHookOn(hookOn),
    HookNamespace: hexNamespace(namespace),
    HookApiVersion: 0,
    ...(hookParameters ? { HookParameters: hookParameters } : {}),
  } as iHook
}

export interface HookLifecycle {
  /** Omit together with `hookOn` when a suite installs its own hook per test (govern.test.ts). */
  wasmName?: string
  namespace: string
  hookOn?: string[]
  /** A function form can read the wallets `setupClient` just produced (e.g. `bob`'s address). */
  hookParameters?: HookParam[] | ((ctx: XrplIntegrationTestContext) => HookParam[])
  /** Defaults to `hook1`. */
  wallet?: (ctx: XrplIntegrationTestContext) => Wallet
  /** Runs `clearHookState` (hsfNSDelete) before `clearAllHooks`. */
  clearState?: boolean
  /** Swallows a `clearAllHooks` failure (the master account's known hook-deletion quirk). */
  ignoreClearErrors?: boolean
}

/**
 * Registers the `beforeAll`/`afterAll` pair every suite repeats: deploy the
 * hook (unless `wasmName`/`hookOn` are omitted), then tear it down. Returns
 * a getter for the live `testContext`.
 */
export function installHook(config: HookLifecycle): () => XrplIntegrationTestContext {
  let testContext: XrplIntegrationTestContext
  const walletOf = config.wallet ?? ((ctx: XrplIntegrationTestContext) => ctx.hook1)

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)
    if (config.wasmName && config.hookOn) {
      const hookParameters =
        typeof config.hookParameters === 'function'
          ? config.hookParameters(testContext)
          : config.hookParameters
      await setHooks({
        client: testContext.client,
        wallet: walletOf(testContext),
        hooks: [{ Hook: buildHook(config.wasmName, config.namespace, config.hookOn, hookParameters) }],
      })
    }
  })

  afterAll(async () => {
    const wallet = walletOf(testContext)
    if (config.clearState) {
      await clearHookState({
        client: testContext.client,
        wallet,
        hooks: [
          {
            Hook: {
              Flags: HookFlags.hsfNSDelete,
              HookNamespace: hexNamespace(config.namespace),
            } as iHook,
          },
        ],
      })
    }
    const teardownHooks = () => clearAllHooks({ client: testContext.client, wallet })
    if (config.ignoreClearErrors) {
      await teardownHooks().catch((e) =>
        console.warn(`${config.namespace}: clearAllHooks failed (ignored) -`, e),
      )
    } else {
      await teardownHooks()
    }
    await teardownClient(testContext)
  })

  return () => testContext
}

/** The static worst-case instruction count CI gates on (`wce.hook` in `examples/<dir>/metrics.json`). */
export function readWorstCaseHook(exampleDir: string, hookFn = 'main'): number {
  const metrics = JSON.parse(
    readFileSync(join(repoRoot, 'examples', exampleDir, 'metrics.json'), 'utf8'),
  )
  const entry = (metrics.entries as Array<{ hook_fn: string; wce: { hook: number } }>).find(
    (e) => e.hook_fn === hookFn,
  )
  if (!entry) {
    throw new Error(`metrics.json: no entry for hook_fn "${hookFn}" in ${exampleDir}`)
  }
  return entry.wce.hook
}

/** The generated `sethook.template.json`'s own `HookParameters` declaration array. */
export function readDeclaredHookParameters(exampleDir: string): HookParam[] {
  const templatePath = join(repoRoot, 'examples', exampleDir, 'out', 'current', 'sethook.template.json')
  const template = JSON.parse(readFileSync(templatePath, 'utf8'))
  return template.Hooks[0].Hook.HookParameters
}
