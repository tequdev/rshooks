import {
  ExecutionUtility,
  Xrpld,
  clearAllHooks,
  hexNamespace,
  readHookBinaryHexFromNS,
  serverUrl,
  setHooks,
  setupClient,
  teardownClient,
  type XrplIntegrationTestContext,
  type iHook,
} from '@xahau/hooks-toolkit'
import { calculateHookOn } from 'xahau'
import { HookFlags } from 'xahau/dist/npm/models/common/xahau'

const namespace = 'rshooks-e2e-reward'
// The hook's static worst case, from
// out/current/1.reward.metadata.json (WCE.hook).
const WORST_CASE_HOOK_INSTRUCTIONS = 12881

describe('reward', () => {
  let testContext: XrplIntegrationTestContext

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)

    const hook: iHook = {
      CreateCode: readHookBinaryHexFromNS('reward', 'wasm'),
      Flags: HookFlags.hsfOverride,
      HookOn: calculateHookOn(['Invoke', 'ClaimReward']),
      HookNamespace: hexNamespace(namespace),
      HookApiVersion: 0,
    }
    await setHooks({
      client: testContext.client,
      wallet: testContext.hook1,
      hooks: [{ Hook: hook }],
    })
  })

  afterAll(async () => {
    await clearAllHooks({
      client: testContext.client,
      wallet: testContext.hook1,
    })
    await teardownClient(testContext)
  })

  it('passes through a non-ClaimReward txn (reward.c L106)', async () => {
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
