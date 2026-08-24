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
import { calculateHookOn, convertStringToHex, decodeAccountID } from 'xahau'
import { HookFlags } from 'xahau/dist/npm/models/common/xahau'

type Wallet = XrplIntegrationTestContext['alice']

const namespace = 'rshooks-e2e-govern'
// The hook's static worst case, from
// out/current/0.govern.metadata.json (WCE.hook).
const WORST_CASE_HOOK_INSTRUCTIONS = 43082

function accountIdHex(classicAddress: string): string {
  return Buffer.from(decodeAccountID(classicAddress)).toString('hex').toUpperCase()
}

function hookParam(name: string, valueHex: string) {
  return {
    HookParameter: {
      HookParameterName: convertStringToHex(name),
      HookParameterValue: valueHex,
    },
  }
}

function isParam(seat: number, wallet: Wallet) {
  return {
    HookParameter: {
      HookParameterName: Buffer.from([0x49, 0x53, seat]).toString('hex').toUpperCase(),
      HookParameterValue: accountIdHex(wallet.classicAddress),
    },
  }
}

async function installGovern(
  testContext: XrplIntegrationTestContext,
  table: Wallet,
  members: Wallet[],
  extra: ReturnType<typeof hookParam>[] = [],
) {
  const hook: iHook = {
    CreateCode: readHookBinaryHexFromNS('govern', 'wasm'),
    Flags: HookFlags.hsfOverride,
    HookOn: calculateHookOn(['Invoke']),
    HookNamespace: hexNamespace(namespace),
    HookApiVersion: 0,
    HookParameters: [
      hookParam('IMC', members.length.toString(16).padStart(2, '0')),
      ...members.map((m, i) => isParam(i, m)),
      ...extra,
    ],
  } as iHook
  await setHooksV3({
    client: testContext.client,
    seed: table.seed,
    hooks: [{ Hook: hook }],
  } as unknown as SetHookParams)
}

async function invoke(
  testContext: XrplIntegrationTestContext,
  from: Wallet,
  table: Wallet,
  params: ReturnType<typeof hookParam>[] = [],
) {
  return Xrpld.submit(testContext.client, {
    tx: {
      TransactionType: 'Invoke',
      Account: from.classicAddress,
      Destination: table.classicAddress,
      HookParameters: params,
    } as any,
    wallet: from,
  })
}

function topicParam(topicType: string, topicId: number) {
  return hookParam('T', Buffer.from([topicType.charCodeAt(0), topicId]).toString('hex').toUpperCase())
}

function voteParam(valueHex: string) {
  return hookParam('V', valueHex)
}

function layerParam(layer: number) {
  return hookParam('L', Buffer.from([layer]).toString('hex').toUpperCase())
}

describe('govern: L2 table setup', () => {
  let testContext: XrplIntegrationTestContext

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)
  })

  afterAll(async () => {
    await clearAllHooksV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
    } as unknown as SetHookParams)
    await teardownClient(testContext)
  })

  it('first Invoke on a fresh table populates the seat table and accepts', async () => {
    await installGovern(testContext, testContext.hook1, [
      testContext.alice,
      testContext.bob,
      testContext.carol,
    ])

    const response = await invoke(testContext, testContext.alice, testContext.hook1)
    const meta = response.meta as any
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(testContext.client, meta)
    expect(hookExecutions.executions.length).toBe(1)
    const execution = hookExecutions.executions[0]
    expect(execution.HookReturnString).toBe('Governance: Setup completed successfully.')
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_HOOK_INSTRUCTIONS,
    )
  })
})

describe('govern: L2 table seat voting', () => {
  let testContext: XrplIntegrationTestContext

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)
    await installGovern(testContext, testContext.hook1, [
      testContext.alice,
      testContext.bob,
      testContext.carol,
    ])
    await invoke(testContext, testContext.alice, testContext.hook1)
  })

  afterAll(async () => {
    await clearAllHooksV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
    } as unknown as SetHookParams)
    await teardownClient(testContext)
  })

  it('a single vote below the 80% seat threshold (2 of 3) just records', async () => {
    const response = await invoke(testContext, testContext.alice, testContext.hook1, [
      topicParam('S', 2),
      voteParam(accountIdHex(testContext.dave.classicAddress)),
      layerParam(2),
    ])
    const meta = response.meta as any
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(testContext.client, meta)
    expect(hookExecutions.executions[0].HookReturnString).toBe(
      'Governance: Vote record. Not yet enough votes to action.',
    )
  })

  it('a second vote reaches the threshold and actions the seat change', async () => {
    const response = await invoke(testContext, testContext.bob, testContext.hook1, [
      topicParam('S', 2),
      voteParam(accountIdHex(testContext.dave.classicAddress)),
      layerParam(2),
    ])
    const meta = response.meta as any
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(testContext.client, meta)
    expect(hookExecutions.executions[0].HookReturnString).toBe('Governance: Action member change.')
  })

  it('casting the identical vote again is a no-op accept', async () => {
    const response = await invoke(testContext, testContext.bob, testContext.hook1, [
      topicParam('S', 2),
      voteParam(accountIdHex(testContext.dave.classicAddress)),
      layerParam(2),
    ])
    const meta = response.meta as any
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(testContext.client, meta)
    expect(hookExecutions.executions[0].HookReturnString).toBe(
      'Governance: Your vote is already cast this way for this topic.',
    )
  })
})

describe('govern: L1 table (real genesis account) reward-rate vote', () => {
  let testContext: XrplIntegrationTestContext

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)
  })

  afterAll(async () => {
    await clearAllHooksV3({
      client: testContext.client,
      seed: testContext.master.seed,
    } as unknown as SetHookParams)
    await teardownClient(testContext)
  })

  it('installs on the real genesis account and completes L1 setup', async () => {
    await installGovern(
      testContext,
      testContext.master,
      [testContext.alice, testContext.bob, testContext.carol],
      [
        hookParam('IRR', '0000000000000000'),
        hookParam('IRD', '0100000000000000'),
      ],
    )

    const response = await invoke(testContext, testContext.alice, testContext.master)
    const meta = response.meta as any
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(testContext.client, meta)
    expect(hookExecutions.executions[0].HookReturnString).toBe(
      'Governance: Setup completed successfully.',
    )
  })

  it('a unanimous RR vote (3 of 3, 100% required at L1) actions the reward rate', async () => {
    const rrValue = '0100000000000000'
    await invoke(testContext, testContext.alice, testContext.master, [
      topicParam('R', 'R'.charCodeAt(0)),
      voteParam(rrValue),
    ])
    await invoke(testContext, testContext.bob, testContext.master, [
      topicParam('R', 'R'.charCodeAt(0)),
      voteParam(rrValue),
    ])
    const response = await invoke(testContext, testContext.carol, testContext.master, [
      topicParam('R', 'R'.charCodeAt(0)),
      voteParam(rrValue),
    ])
    const meta = response.meta as any
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(testContext.client, meta)
    expect(hookExecutions.executions[0].HookReturnString).toBe(
      'Governance: Reward rate change actioned!',
    )
  })
})

describe('govern: L1 table (real genesis account) — intentional IRR/IRD length-strictness divergence', () => {
  let testContext: XrplIntegrationTestContext

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)
  })

  afterAll(async () => {
    await clearAllHooksV3({
      client: testContext.client,
      seed: testContext.master.seed,
    } as unknown as SetHookParams)
    await teardownClient(testContext)
  })

  it('rejects a too-short IRR value at setup instead of silently zero-padding it (govern.c would accept it)', async () => {
    await installGovern(
      testContext,
      testContext.master,
      [testContext.alice, testContext.bob, testContext.carol],
      [
        hookParam('IRR', '000000'), // 3 bytes, not the expected 8
        hookParam('IRD', '0100000000000000'),
      ],
    )

    const response = invoke(testContext, testContext.alice, testContext.master)
    await expect(response).rejects.toThrow(
      'Governance: Initial Reward Rate Parameter missing (IRR).',
    )
  })
})
