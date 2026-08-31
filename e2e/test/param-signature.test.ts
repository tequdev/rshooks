// e2e for `examples/19_param-signature`: `increment(account: AccountID,
// count: UInt16)`, the Hook Parameter Signature Interface draft's own
// worked example (docs/PARAM_SIGNATURE_DESIGN.md). Unlike every other
// suite here, this hook is installed using the *generated*
// `sethook.template.json`'s own `HookParameters` declaration entries,
// read straight off disk - proving the template `rshooks build` writes
// round-trips through an actual SetHook, not just that its shape looks
// right in isolation (see book/src/build/metadata.md's "Generated
// `SetHook` declarations" section).

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  ExecutionUtility,
  StateUtility,
  Xrpld,
  clearAllHooksV3,
  clearHookStateV3,
  hexNamespace,
  readHookBinaryHexFromNS,
  serverUrl,
  setHooksV3,
  setupClient,
  teardownClient,
  type SetHookParams,
  type XrplIntegrationTestContext,
  type iHook,
} from '@transia/hooks-toolkit'
import { calculateHookOn, decodeAccountID, type TransactionMetadata } from 'xahau'
import { HookFlags } from 'xahau/dist/npm/models/common/xahau'
import { sigParam, u16BEHex } from './sig-param'

const namespace = 'rshooks-e2e-param-signature'
const WORST_CASE_INSTRUCTIONS = 280

// This file lives in `e2e/test/`, mirroring `e2e/scripts/copy-wasm.mjs`'s
// own two-level walk up to the repo root.
const e2eRoot = dirname(dirname(fileURLToPath(import.meta.url)))
const repoRoot = dirname(e2eRoot)
const templatePath = join(
  repoRoot,
  'examples',
  '19_param-signature',
  'out',
  'current',
  'sethook.template.json',
)

// The generated template's own `HookParameters` declaration array for this
// entry - `[{ HookParameter: { HookParameterName, HookParameterValue: "00" } }, ...]`,
// in index order: `account`(0) then `count`(1). Installed verbatim below.
const template = JSON.parse(readFileSync(templatePath, 'utf8'))
const declaredHookParameters = template.Hooks[0].Hook.HookParameters as Array<{
  HookParameter: { HookParameterName: string; HookParameterValue: string }
}>
const ACCOUNT_NAME_HEX = declaredHookParameters[0].HookParameter.HookParameterName
const COUNT_NAME_HEX = declaredHookParameters[1].HookParameter.HookParameterName

function accountIdHex(address: string): string {
  return Buffer.from(decodeAccountID(address)).toString('hex').toUpperCase()
}

// Hook state keys are left-padded to 32 bytes by the host - `CounterKey`
// encodes to exactly the 20-byte account id, no discriminant tag (this
// chain has only one state field; see the example's own README).
function counterStateKeyHex(address: string): string {
  return accountIdHex(address).padStart(64, '0')
}

describe('param-signature', () => {
  let testContext: XrplIntegrationTestContext
  const hookNamespace = hexNamespace(namespace)

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)

    const hook: iHook = {
      CreateCode: readHookBinaryHexFromNS('param_signature', 'wasm'),
      Flags: HookFlags.hsfOverride,
      HookOn: calculateHookOn(['Invoke']),
      HookNamespace: hookNamespace,
      HookApiVersion: 0,
      // Installed verbatim from the generated `sethook.template.json` -
      // both declaration entries, `HookParameterValue = "00"`.
      HookParameters: declaredHookParameters,
    } as iHook
    await setHooksV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
      hooks: [{ Hook: hook }],
    } as unknown as SetHookParams)
  })

  afterAll(async () => {
    const clearStateHook: iHook = {
      Flags: HookFlags.hsfNSDelete,
      HookNamespace: hookNamespace,
    }
    await clearHookStateV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
      hooks: [{ Hook: clearStateHook }],
    } as unknown as SetHookParams)
    await clearAllHooksV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
    } as unknown as SetHookParams)
    await teardownClient(testContext)
  })

  function invoke(
    sender: XrplIntegrationTestContext['alice'],
    target: string,
    count: number,
  ) {
    return Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Invoke',
        Account: sender.classicAddress,
        Destination: testContext.hook1.classicAddress,
        HookParameters: [
          sigParam(ACCOUNT_NAME_HEX, accountIdHex(target)),
          sigParam(COUNT_NAME_HEX, u16BEHex(count)),
        ],
      } as any,
      wallet: sender,
    })
  }

  it('the generated template declares exactly the two signature parameters', () => {
    expect(declaredHookParameters.length).toBe(2)
    expect(declaredHookParameters[0].HookParameter.HookParameterValue).toBe('00')
    expect(declaredHookParameters[1].HookParameter.HookParameterValue).toBe('00')
  })

  it('accepts a well-formed Invoke and returns the new count as the accept code', async () => {
    const response = await invoke(testContext.alice, testContext.alice.classicAddress, 5)

    const meta = response.meta as TransactionMetadata
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
      testContext.client,
      meta,
    )
    expect(hookExecutions.executions.length).toBe(1)
    const execution = hookExecutions.executions[0]
    // `accept!(b"param-signature: incremented", next as i64)` - the resulting
    // counter total (alice's first invocation, so exactly `count`) becomes
    // the `HookReturnCode`, a hex string over RPC (see typed-data.test.ts
    // for the same convention).
    expect(BigInt(`0x${execution.HookReturnCode}`)).toBe(5n)
    expect(execution.HookReturnString).toBe('param-signature: incremented')
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_INSTRUCTIONS,
    )
  })

  it('persists the per-account counter as an 8-byte LE u64 in hook state', async () => {
    const entry = await StateUtility.getHookState(
      testContext.client,
      testContext.hook1.classicAddress,
      counterStateKeyHex(testContext.alice.classicAddress),
      hookNamespace,
    )
    const raw = Buffer.from(entry.HookStateData, 'hex')
    expect(raw.length).toBe(8)
    expect(raw.readBigUInt64LE(0)).toBe(5n)
  })

  it('accumulates across invocations, keyed per account', async () => {
    const response = await invoke(testContext.alice, testContext.alice.classicAddress, 3)
    const meta = response.meta as TransactionMetadata
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
      testContext.client,
      meta,
    )
    expect(BigInt(`0x${hookExecutions.executions[0].HookReturnCode}`)).toBe(8n)
  })

  it("bob's own counter is independent of alice's", async () => {
    const response = await invoke(testContext.bob, testContext.bob.classicAddress, 1)
    const meta = response.meta as TransactionMetadata
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
      testContext.client,
      meta,
    )
    expect(BigInt(`0x${hookExecutions.executions[0].HookReturnCode}`)).toBe(1n)
  })

  it('rejects an Invoke missing the count signature parameter', async () => {
    const response = Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Invoke',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
        HookParameters: [
          sigParam(ACCOUNT_NAME_HEX, accountIdHex(testContext.alice.classicAddress)),
        ],
      } as any,
      wallet: testContext.alice,
    })
    await expect(response).rejects.toThrow("rshooks: bad sig param 'count'")
  })

  it('rejects an Invoke with a short (wrong-length) count value', async () => {
    const response = Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Invoke',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
        HookParameters: [
          sigParam(ACCOUNT_NAME_HEX, accountIdHex(testContext.alice.classicAddress)),
          // `count` decodes as `u16` (2 bytes BE) - one byte is too short.
          sigParam(COUNT_NAME_HEX, '07'),
        ],
      } as any,
      wallet: testContext.alice,
    })
    await expect(response).rejects.toThrow("rshooks: bad sig param 'count'")
  })
})
