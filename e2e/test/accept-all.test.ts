import { ExecutionUtility, Xrpld } from '@xahau/hooks-toolkit'
import type { TransactionMetadata } from 'xahau'
import { installHook, readWorstCaseHook } from './harness'

const WORST_CASE_INSTRUCTIONS = readWorstCaseHook('01_accept-all')

describe('accept-all', () => {
  const getContext = installHook({
    wasmName: 'accept_all',
    namespace: 'rshooks-e2e-accept-all',
    hookOn: ['Invoke'],
  })

  it('accepts an Invoke with an empty return string and code 0', async () => {
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
