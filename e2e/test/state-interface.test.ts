// e2e for `examples/20_state-interface`: `balances(id=0):
// key(account: AccountId, token: u32), value(amount: u64, updated: u32)`
// and `config(id=1): value(paused: u8)`, the Hook State Interface draft's
// own worked example (docs/STATE_INTERFACE_DESIGN.md). Installed using the
// *generated* `sethook.template.json`'s own `HookParameters` declaration
// entries, read straight off disk (mirrors param-signature.test.ts), then
// asserts the live on-ledger `HookStateKey`/`HookStateData` bytes match the
// design doc's own §7 spec vector shape for the keyed entry, plus the
// singleton entry's key/value shape.

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

const namespace = 'rshooks-e2e-state-interface'
const WORST_CASE_INSTRUCTIONS = 374

// This file lives in `e2e/test/`, mirroring `e2e/scripts/copy-wasm.mjs`'s
// own two-level walk up to the repo root.
const e2eRoot = dirname(dirname(fileURLToPath(import.meta.url)))
const repoRoot = dirname(e2eRoot)
const templatePath = join(
  repoRoot,
  'examples',
  '20_state-interface',
  'out',
  'current',
  'sethook.template.json',
)

// The generated template's own `HookParameters` declaration array for this
// entry - `balances`(id 0) then `config`(id 1), each with the real value
// schema as `HookParameterValue` (not a "00" marker - see
// docs/STATE_INTERFACE_DESIGN.md §5). Installed verbatim below.
const template = JSON.parse(readFileSync(templatePath, 'utf8'))
const declaredHookParameters = template.Hooks[0].Hook.HookParameters as Array<{
  HookParameter: { HookParameterName: string; HookParameterValue: string }
}>

// The keyed `balances` entry's `HookStateKey`
// (docs/STATE_INTERFACE_DESIGN.md §1.6): State ID (1 byte) || account (20
// bytes) || token (4 bytes BE) || zero padding to 32 bytes total. `token`
// is fixed to `0` in this example (one balance per sender).
function balanceStateKeyHex(address: string): string {
  const key = Buffer.alloc(32)
  key[0] = 0x00 // State ID
  Buffer.from(decodeAccountID(address)).copy(key, 1)
  key.writeUInt32BE(0, 21) // token = 0
  return key.toString('hex').toUpperCase()
}

// The singleton `config` entry's `HookStateKey`: State ID `0x01` followed
// by 31 zero bytes.
const CONFIG_STATE_KEY_HEX = '01'.padEnd(64, '0')

// The keyed `balances` entry's on-ledger `HookStateData`
// (docs/STATE_INTERFACE_DESIGN.md §1.7): `amount` (8 bytes BE) then
// `updated` (4 bytes BE), concatenated directly - no field-count prefix or
// separators, unlike the *declaration* `HookParameterValue` above.
function balanceStateDataHex(amount: bigint, updated: number): string {
  const buf = Buffer.alloc(12)
  buf.writeBigUInt64BE(amount, 0)
  buf.writeUInt32BE(updated, 8)
  return buf.toString('hex').toLowerCase()
}

describe('state-interface', () => {
  let testContext: XrplIntegrationTestContext
  const hookNamespace = hexNamespace(namespace)

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)

    const hook: iHook = {
      CreateCode: readHookBinaryHexFromNS('state_interface', 'wasm'),
      Flags: HookFlags.hsfOverride,
      HookOn: calculateHookOn(['Invoke']),
      HookNamespace: hookNamespace,
      HookApiVersion: 0,
      // Installed verbatim from the generated template (see header comment).
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

  function invoke(sender: XrplIntegrationTestContext['alice']) {
    return Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Invoke',
        Account: sender.classicAddress,
        Destination: testContext.hook1.classicAddress,
      } as any,
      wallet: sender,
    })
  }

  it('the generated template declares both state interface entries with real value schemas', () => {
    expect(declaredHookParameters.length).toBe(2)
    expect(declaredHookParameters[0].HookParameter.HookParameterName).toBe(
      '5F534900000208076163636F756E740205746F6B656E',
    )
    expect(declaredHookParameters[0].HookParameter.HookParameterValue).toBe(
      '020306616D6F756E74020775706461746564',
    )
    expect(declaredHookParameters[1].HookParameter.HookParameterName).toBe('5F5349000100')
    expect(declaredHookParameters[1].HookParameter.HookParameterValue).toBe(
      '011006706175736564',
    )
  })

  it('accepts an Invoke and returns the new balance as the accept code', async () => {
    const response = await invoke(testContext.alice)

    const meta = response.meta as TransactionMetadata
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
      testContext.client,
      meta,
    )
    expect(hookExecutions.executions.length).toBe(1)
    const execution = hookExecutions.executions[0]
    expect(BigInt(`0x${execution.HookReturnCode}`)).toBe(1n)
    expect(execution.HookReturnString).toBe('state-interface: credited')
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_INSTRUCTIONS,
    )
  })

  it('persists the keyed balance as amount=1, updated=1, both big-endian, no field-count prefix', async () => {
    const entry = await StateUtility.getHookState(
      testContext.client,
      testContext.hook1.classicAddress,
      balanceStateKeyHex(testContext.alice.classicAddress),
      hookNamespace,
    )
    expect(entry.HookStateData.toLowerCase()).toBe(balanceStateDataHex(1n, 1))
  })

  it('persists the singleton config entry as paused=0', async () => {
    const entry = await StateUtility.getHookState(
      testContext.client,
      testContext.hook1.classicAddress,
      CONFIG_STATE_KEY_HEX,
      hookNamespace,
    )
    expect(entry.HookStateData).toBe('00')
  })

  it('accumulates across invocations, keyed per (account, token)', async () => {
    const response = await invoke(testContext.alice)
    const meta = response.meta as TransactionMetadata
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
      testContext.client,
      meta,
    )
    expect(BigInt(`0x${hookExecutions.executions[0].HookReturnCode}`)).toBe(2n)

    const entry = await StateUtility.getHookState(
      testContext.client,
      testContext.hook1.classicAddress,
      balanceStateKeyHex(testContext.alice.classicAddress),
      hookNamespace,
    )
    expect(entry.HookStateData.toLowerCase()).toBe(balanceStateDataHex(2n, 2))
  })

  it("bob's own balance is independent of alice's", async () => {
    const response = await invoke(testContext.bob)
    const meta = response.meta as TransactionMetadata
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
      testContext.client,
      meta,
    )
    expect(BigInt(`0x${hookExecutions.executions[0].HookReturnCode}`)).toBe(1n)
  })
})
