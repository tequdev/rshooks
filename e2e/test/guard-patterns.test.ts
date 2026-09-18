import { ExecutionUtility, Xrpld } from '@xahau/hooks-toolkit'
import { convertStringToHex, decodeAccountID, type TransactionMetadata } from 'xahau'
import { installHook } from './harness'

const WORST_CASE_INSTRUCTIONS = 615

function accountIdHex(classicAddress: string): string {
  return Buffer.from(decodeAccountID(classicAddress)).toString('hex').toUpperCase()
}

describe('guard-patterns', () => {
  const getContext = installHook({
    wasmName: 'guard_patterns',
    namespace: 'rshooks-e2e-guard-patterns',
    hookOn: ['Invoke'],
    hookParameters: (ctx) => [
      {
        HookParameter: {
          HookParameterName: convertStringToHex('BL'),
          HookParameterValue: accountIdHex(ctx.bob.classicAddress),
        },
      },
    ],
  })

  it('rejects an Invoke from the blocked account', async () => {
    const testContext = getContext()
    const response = Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Invoke',
        Account: testContext.bob.classicAddress,
        Destination: testContext.hook1.classicAddress,
      },
      wallet: testContext.bob,
    })
    await expect(response).rejects.toThrow('guard-patterns: blocked account')
  })

  it('accepts an Invoke from a non-blocked account', async () => {
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
    expect(execution.HookReturnString).toBe('guard-patterns: accepted')
    // Hook instruction counts are hexadecimal RPC values.
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_INSTRUCTIONS,
    )
  })
})
