import { ExecutionUtility, Xrpld } from '@xahau/hooks-toolkit'
import { installHook } from './harness'

// The hook's static worst case, from
// out/current/1.reward.metadata.json (WCE.hook).
const WORST_CASE_HOOK_INSTRUCTIONS = 12881

describe('reward', () => {
  const getContext = installHook({
    wasmName: 'reward',
    namespace: 'rshooks-e2e-reward',
    hookOn: ['Invoke', 'ClaimReward'],
  })

  it('passes through a non-ClaimReward txn (reward.c L106)', async () => {
    const testContext = getContext()
    const response = await Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Invoke',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
      },
      wallet: testContext.alice,
    })

    const meta = response.meta as any
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(testContext.client, meta)
    expect(hookExecutions.executions.length).toBe(1)
    const execution = hookExecutions.executions[0]
    expect(execution.HookReturnString).toBe('Reward: Passing non-claim txn')
    expect(Number(execution.HookReturnCode)).toBe(0)
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_HOOK_INSTRUCTIONS,
    )
  })
})
