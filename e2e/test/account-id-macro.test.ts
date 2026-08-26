import {
  ExecutionUtility,
  Xrpld,
  clearAllHooksV3,
  hexNamespace,
  readHookBinaryHexFromNS,
  serverUrl,
  setHooksV3,
  setupClient,
  teardownClient,
  type SetHookParams,
  type XrplIntegrationTestContext,
  type iHook,
} from '@transia/hooks-toolkit'
import { calculateHookOn } from 'xahau'
import { HookFlags } from 'xahau/dist/npm/models/common/xahau'

const namespace = 'rshooks-e2e-account-id-macro'
const WORST_CASE_HOOK_INSTRUCTIONS = 245

describe('account-id-macro', () => {
  let testContext: XrplIntegrationTestContext

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)

    const hook: iHook = {
      CreateCode: readHookBinaryHexFromNS('account_id_macro', 'wasm'),
      Flags: HookFlags.hsfOverride,
      HookOn: calculateHookOn(['Invoke']),
      HookNamespace: hexNamespace(namespace),
      HookApiVersion: 0,
    }
    await setHooksV3({
      client: testContext.client,
      seed: testContext.master.seed,
      hooks: [{ Hook: hook }],
    } as unknown as SetHookParams)
  })

  afterAll(async () => {
    // Hook deletion on the standalone master account can return `tefINTERNAL`.
    try {
      await clearAllHooksV3({
        client: testContext.client,
        seed: testContext.master.seed,
      } as unknown as SetHookParams)
    } catch (e) {
      console.warn(
        'account-id-macro: clearAllHooksV3 on master failed (known xahaud ' +
          'master-account hook-deletion failure) - ' +
          'ignoring:',
        e,
      )
    }
    await teardownClient(testContext)
  })

  it('hook_account/util_accid/util_raddr all agree with the account_id! compile-time constant', async () => {
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
