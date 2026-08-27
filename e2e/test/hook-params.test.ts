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
import { calculateHookOn, convertStringToHex, type TransactionMetadata } from 'xahau'
import { HookFlags } from 'xahau/dist/npm/models/common/xahau'

const namespace = 'rshooks-e2e-hook-params'
const WORST_CASE_INSTRUCTIONS = 172
const MIN_DROPS = 5_000_000n

function u64LEHex(value: bigint): string {
  const buf = Buffer.alloc(8)
  buf.writeBigUInt64LE(value)
  return buf.toString('hex').toUpperCase()
}

describe('hook-params', () => {
  let testContext: XrplIntegrationTestContext

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)

    const hook: iHook = {
      CreateCode: readHookBinaryHexFromNS('hook_params', 'wasm'),
      Flags: HookFlags.hsfOverride,
      HookOn: calculateHookOn(['Payment']),
      HookNamespace: hexNamespace(namespace),
      HookApiVersion: 0,
      HookParameters: [
        {
          HookParameter: {
            HookParameterName: convertStringToHex('MIN'),
            HookParameterValue: u64LEHex(MIN_DROPS),
          },
        },
      ],
    } as iHook
    await setHooksV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
      hooks: [{ Hook: hook }],
    } as unknown as SetHookParams)
  })

  afterAll(async () => {
    await clearAllHooksV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
    } as unknown as SetHookParams)
    await teardownClient(testContext)
  })

  it('rejects a Payment below the configured MIN threshold', async () => {
    const response = Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Payment',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
        Amount: '1000000', // 1 XAH, below the configured 5 XAH minimum
      },
      wallet: testContext.alice,
    })
    await expect(response).rejects.toThrow(
      'hook-params: amount below configured minimum',
    )
  })

  it('accepts a Payment at or above the configured MIN threshold', async () => {
    const response = await Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Payment',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
        Amount: '6000000', // 6 XAH, above the configured 5 XAH minimum
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
