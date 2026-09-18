import { ExecutionUtility, Xrpld, setHooks, type XrplIntegrationTestContext } from '@xahau/hooks-toolkit'
import { convertStringToHex, decodeAccountID } from 'xahau'
import { buildHook, installHook, readWorstCaseHook, type Wallet } from './harness'

const namespace = 'rshooks-e2e-govern'
const WORST_CASE_HOOK_INSTRUCTIONS = readWorstCaseHook('80_governance', 'govern')

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
  await setHooks({
    client: testContext.client,
    wallet: table,
    hooks: [
      {
        Hook: buildHook('govern', namespace, ['Invoke'], [
          hookParam('IMC', members.length.toString(16).padStart(2, '0')),
          ...members.map((m, i) => isParam(i, m)),
          ...extra,
        ]),
      },
    ],
  })
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
  const getContext = installHook({ namespace, wallet: (ctx) => ctx.hook1 })

  it('first Invoke on a fresh table populates the seat table and accepts', async () => {
    const testContext = getContext()
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
  const getContext = installHook({ namespace, wallet: (ctx) => ctx.hook1 })

  beforeAll(async () => {
    const testContext = getContext()
    await installGovern(testContext, testContext.hook1, [
      testContext.alice,
      testContext.bob,
      testContext.carol,
    ])
    await invoke(testContext, testContext.alice, testContext.hook1)
  })

  it('a single vote below the 80% seat threshold (2 of 3) just records', async () => {
    const testContext = getContext()
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
    const testContext = getContext()
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
    const testContext = getContext()
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
  const getContext = installHook({ namespace, wallet: (ctx) => ctx.master })

  it('installs on the real genesis account and completes L1 setup', async () => {
    const testContext = getContext()
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
    const testContext = getContext()
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
  const getContext = installHook({ namespace, wallet: (ctx) => ctx.master })

  it('rejects a too-short IRR value at setup instead of silently zero-padding it (govern.c would accept it)', async () => {
    const testContext = getContext()
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
