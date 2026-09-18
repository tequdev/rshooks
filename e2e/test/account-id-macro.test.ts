import { ExecutionUtility, Xrpld } from '@xahau/hooks-toolkit'
import { installHook, readWorstCaseHook } from './harness'

const WORST_CASE_HOOK_INSTRUCTIONS = readWorstCaseHook('14_account-id-macro')

describe('account-id-macro', () => {
  const getContext = installHook({
    wasmName: 'account_id_macro',
    namespace: 'rshooks-e2e-account-id-macro',
    hookOn: ['Invoke'],
    wallet: (ctx) => ctx.master,
    // Hook deletion on the standalone master account can return `tefINTERNAL`.
    ignoreClearErrors: true,
  })

  it('hook_account/util_accid/util_raddr all agree with the account_id! compile-time constant', async () => {
    const testContext = getContext()
    const response = await Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Invoke',
        Account: testContext.alice.classicAddress,
        Destination: testContext.master.classicAddress,
      },
      wallet: testContext.alice,
    })

    const meta = response.meta as any
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
      testContext.client,
      meta,
    )
    expect(hookExecutions.executions.length).toBe(1)
    const execution = hookExecutions.executions[0]
    expect(Number(execution.HookReturnCode)).toBe(0)
    expect(execution.HookReturnString).toBe(
      'account-id-macro: all three checks passed',
    )
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_HOOK_INSTRUCTIONS,
    )
  })
})
