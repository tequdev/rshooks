import { ExecutionUtility, Xrpld } from '@xahau/hooks-toolkit'
import type { TransactionMetadata } from 'xahau'
import { installHook, readWorstCaseHook } from './harness'

const WORST_CASE_INSTRUCTIONS = readWorstCaseHook('07_xfl-math')

describe('xfl-math', () => {
  const getContext = installHook({
    wasmName: 'xfl_math',
    namespace: 'rshooks-e2e-xfl-math',
    hookOn: ['Payment'],
  })

  it('rejects a Payment whose computed 1% share falls below the minimum', async () => {
    const testContext = getContext()
    const response = Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Payment',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
        Amount: '50', // 1% of 50 drops < 0.000001 XAH
      },
      wallet: testContext.alice,
    })
    await expect(response).rejects.toThrow('xfl-math: computed share below minimum')
  })

  it('accepts a Payment whose computed 1% share meets the minimum', async () => {
    const testContext = getContext()
    const response = await Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Payment',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
        Amount: '1000000', // 1 XAH; 1% share is 0.01 XAH, well above the minimum
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
    expect(Number(execution.HookReturnCode)).toBe(0)
    expect(execution.HookReturnString).toBe('')
    // Hook instruction counts are hexadecimal RPC values.
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_INSTRUCTIONS,
    )
  })
})
