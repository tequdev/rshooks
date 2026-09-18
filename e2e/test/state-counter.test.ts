import { ExecutionUtility, StateUtility, Xrpld, hexNamespace } from '@xahau/hooks-toolkit'
import type { TransactionMetadata } from 'xahau'
import { installHook, readWorstCaseHook } from './harness'

const namespace = 'rshooks-e2e-state-counter'
const hookNamespace = hexNamespace(namespace)
const WORST_CASE_INSTRUCTIONS = readWorstCaseHook('02_state-counter')

// Hook state keys are left-padded to 32 bytes by the host.
const COUNTER_KEY = Buffer.from('counter', 'ascii')
  .toString('hex')
  .toUpperCase()
  .padStart(64, '0')

describe('state-counter', () => {
  const getContext = installHook({
    wasmName: 'state_counter',
    namespace,
    hookOn: ['Invoke'],
    // Clear persistent state so reruns start at zero.
    clearState: true,
  })

  const invoke = async () => {
    const testContext = getContext()
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
    const testContext = getContext()
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
