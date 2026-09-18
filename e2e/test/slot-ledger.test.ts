import { ExecutionUtility, Xrpld } from '@xahau/hooks-toolkit'
import type { TransactionMetadata } from 'xahau'
import { installHook } from './harness'

const WORST_CASE_INSTRUCTIONS = 197

describe('slot-ledger', () => {
  const getContext = installHook({
    wasmName: 'slot_ledger',
    namespace: 'rshooks-e2e-slot-ledger',
    hookOn: ['Payment'],
  })

  it('reads Destination and a native Amount via the Slot API and accepts', async () => {
    const testContext = getContext()
    const response = await Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Payment',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
        Amount: '1',
      },
      wallet: testContext.alice,
    })

    const meta = response.meta as TransactionMetadata
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
      testContext.client,
      meta,
    )
    expect(hookExecutions.executions.length).toBe(1)
    const execution = hookExecutions.executions[0]
    // Hook return codes are hexadecimal RPC values.
    expect(
      parseInt(String(execution.HookReturnCode), 16),
    ).toBeGreaterThanOrEqual(0)
    expect(execution.HookReturnString).toBe(
      'slot-ledger: read Destination and native Amount',
    )
    // Hook instruction counts are hexadecimal RPC values.
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_INSTRUCTIONS,
    )
  })
})
