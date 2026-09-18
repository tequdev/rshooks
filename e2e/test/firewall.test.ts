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
import {
  calculateHookOn,
  convertStringToHex,
  decodeAccountID,
  type TransactionMetadata,
} from 'xahau'
import { HookFlags } from 'xahau/dist/npm/models/common/xahau'

const namespace = 'rshooks-e2e-firewall'
const WORST_CASE_INSTRUCTIONS = 135

function accountIdHex(classicAddress: string): string {
  return Buffer.from(decodeAccountID(classicAddress)).toString('hex').toUpperCase()
}

describe('firewall', () => {
  let testContext: XrplIntegrationTestContext

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)

    const hook = {
      CreateCode: readHookBinaryHexFromNS('firewall', 'wasm'),
      Flags: HookFlags.hsfOverride,
      HookOn: calculateHookOn(['Payment']),
      HookNamespace: hexNamespace(namespace),
      HookApiVersion: 0,
      HookParameters: [
        {
          HookParameter: {
            HookParameterName: convertStringToHex('BL'),
            HookParameterValue: accountIdHex(testContext.bob.classicAddress),
          },
        },
      ],
    } as iHook
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

  it('rejects a Payment from the blocked account', async () => {
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
