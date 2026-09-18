import { ExecutionUtility, Xrpld } from '@xahau/hooks-toolkit'
import { convertStringToHex, decodeAccountID, type TransactionMetadata } from 'xahau'
import { installHook } from './harness'

const WORST_CASE_INSTRUCTIONS = 135

function accountIdHex(classicAddress: string): string {
  return Buffer.from(decodeAccountID(classicAddress)).toString('hex').toUpperCase()
}

describe('firewall', () => {
  const getContext = installHook({
    wasmName: 'firewall',
    namespace: 'rshooks-e2e-firewall',
    hookOn: ['Payment'],
    hookParameters: (ctx) => [
      {
        HookParameter: {
          HookParameterName: convertStringToHex('BL'),
          HookParameterValue: accountIdHex(ctx.bob.classicAddress),
        },
      },
    ],
  })

  it('rejects a Payment from the blocked account', async () => {
    const testContext = getContext()
    const response = Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Payment',
        Account: testContext.bob.classicAddress,
        Destination: testContext.hook1.classicAddress,
        Amount: '1',
      },
      wallet: testContext.bob,
    })
    await expect(response).rejects.toThrow('firewall: blocked account')
  })

  it('accepts a Payment from a non-blocked account', async () => {
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
