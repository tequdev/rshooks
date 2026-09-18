import { ExecutionUtility, Xrpld } from '@xahau/hooks-toolkit'
import type { TransactionMetadata } from 'xahau'
import { installHook, readWorstCaseHook } from './harness'

const WORST_CASE_INSTRUCTIONS = readWorstCaseHook('04_errors')
const BLOCKED_SOURCE_TAG = 13
const MAX_DROPS = 100_000_000

describe('errors', () => {
  const getContext = installHook({
    wasmName: 'errors',
    namespace: 'rshooks-e2e-errors',
    hookOn: ['Payment'],
  })

  it('rejects a Payment with the blocked SourceTag (code -102)', async () => {
    const testContext = getContext()
    const response = Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Payment',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
        Amount: '1',
        SourceTag: BLOCKED_SOURCE_TAG,
      },
      wallet: testContext.alice,
    })
    await expect(response).rejects.toThrow('errors: blocked SourceTag')
  })

  it('rejects a Payment moving more than the policy limit (code -104)', async () => {
    const testContext = getContext()
    const response = Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Payment',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
        Amount: String(MAX_DROPS + 1),
      },
      wallet: testContext.alice,
    })
    await expect(response).rejects.toThrow('errors: amount exceeds policy limit')
  })

  it('accepts a Payment that passes every check', async () => {
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
    expect(Number(execution.HookReturnCode)).toBe(0)
    expect(execution.HookReturnString).toBe('')
    // Hook instruction counts are hexadecimal RPC values.
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_INSTRUCTIONS,
    )
  })
})
