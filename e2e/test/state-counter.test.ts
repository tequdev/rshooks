import {
  ExecutionUtility,
  StateUtility,
  Xrpld,
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
import { calculateHookOn, type TransactionMetadata } from 'xahau'
import { HookFlags } from 'xahau/dist/npm/models/common/xahau'

const namespace = 'rshooks-e2e-state-counter'
const WORST_CASE_INSTRUCTIONS = 254

// Hook state keys are left-padded to 32 bytes by the host.
const COUNTER_KEY = Buffer.from('counter', 'ascii')
  .toString('hex')
  .toUpperCase()
  .padStart(64, '0')

describe('state-counter', () => {
  let testContext: XrplIntegrationTestContext
  const hookNamespace = hexNamespace(namespace)

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)

    const hook: iHook = {
      CreateCode: readHookBinaryHexFromNS('state_counter', 'wasm'),
      Flags: HookFlags.hsfOverride,
      HookOn: calculateHookOn(['Invoke']),
      HookNamespace: hookNamespace,
      HookApiVersion: 0,
    }
    await setHooks({
      client: testContext.client,
      wallet: testContext.hook1,
      hooks: [{ Hook: hook }],
    })
  })

  afterAll(async () => {
    // Clear persistent state so reruns start at zero.
    const clearStateHook: iHook = {
      Flags: HookFlags.hsfNSDelete,
      HookNamespace: hookNamespace,
    }
    await clearHookState({
      client: testContext.client,
      wallet: testContext.hook1,
      hooks: [{ Hook: clearStateHook }],
    })
    await clearAllHooks({
      client: testContext.client,
      wallet: testContext.hook1,
    })
    await teardownClient(testContext)
  })

  const invoke = async () => {
    const response = await Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Invoke',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
      },
      wallet: testContext.alice,
    })
    const meta = response.meta as TransactionMetadata
    return ExecutionUtility.getHookExecutionsFromMeta(testContext.client, meta)
  }

  it('increments the counter on the first Invoke (count = 1)', async () => {
    const hookExecutions = await invoke()
    expect(hookExecutions.executions.length).toBe(1)
    const execution = hookExecutions.executions[0]
    expect(Number(execution.HookReturnCode)).toBe(1)
    expect(execution.HookReturnString).toBe('state-counter: incremented')
    // Hook instruction counts are hexadecimal RPC values.
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_INSTRUCTIONS,
    )
  })

  it('increments the counter on the second Invoke (count = 2)', async () => {
    const hookExecutions = await invoke()
    expect(hookExecutions.executions.length).toBe(1)
    expect(Number(hookExecutions.executions[0].HookReturnCode)).toBe(2)
  })

  it('persists the counter as an 8-byte LE u64 in hook state', async () => {
    const entry = await StateUtility.getHookState(
      testContext.client,
      testContext.hook1.classicAddress,
      COUNTER_KEY,
      hookNamespace,
    )
    const raw = Buffer.from(entry.HookStateData, 'hex')
    expect(raw.length).toBe(8)
    expect(raw.readBigUInt64LE(0)).toBe(2n)
  })
})
