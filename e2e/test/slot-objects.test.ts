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
import {
  calculateHookOn,
  convertStringToHex,
  decodeAccountID,
  type TransactionMetadata,
} from 'xahau'
import { HookFlags } from 'xahau/dist/npm/models/common/xahau'

const namespace = 'rshooks-e2e-slot-objects'
// The hook's static worst case, from out/current/0.main.metadata.json (WCE.hook).
// Live counts run one group per Invoke, so they sit well under this.
const WORST_CASE_INSTRUCTIONS = 62839

const BIT_ACCOUNT_WALK = 1
const BIT_DROPS_ROUNDTRIP = 2
const BIT_PARENT_CLEAR = 4
const BIT_TAKE_LOOP = 8
const BIT_MIDHOP_LOOP = 16
const BIT_DEEP_LOOP = 32
const BIT_TAKE_FAILURE = 64
const BIT_CAST_CLEANUP = 128
const BIT_ROOT_CAST = 256
const BIT_U64_WIRE = 512
const BIT_IOU_XFL = 1024
const ALL_CHECKS =
  BIT_ACCOUNT_WALK |
  BIT_DROPS_ROUNDTRIP |
  BIT_PARENT_CLEAR |
  BIT_TAKE_LOOP |
  BIT_MIDHOP_LOOP |
  BIT_DEEP_LOOP |
  BIT_TAKE_FAILURE |
  BIT_CAST_CLEANUP |
  BIT_ROOT_CAST |
  BIT_U64_WIRE |
  BIT_IOU_XFL

const CHK_CHEAP = 0
const CHK_DEEP = 1
const CHK_TAKE_FAILURE = 2
const CHK_CAST = 3
const CHK_MIDHOP = 4
const CHK_IOU = 5

const IOU_CURRENCY = 'USD'
const IOU_AMOUNT = '100'

describe('slot-objects (typed slot layer)', () => {
  let testContext: XrplIntegrationTestContext
  let checks = 0

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)

    // Install a SignerList so the failing path allocates intermediate slots.
    await Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'SignerListSet',
        Account: testContext.alice.classicAddress,
        SignerQuorum: 1,
        SignerEntries: [
          {
            SignerEntry: {
              Account: testContext.bob.classicAddress,
              SignerWeight: 1,
            },
          },
        ],
      } as never,
      wallet: testContext.alice,
    })

    // Fund a trust line to exercise IOU XFL conversion.
    await Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'TrustSet',
        Account: testContext.alice.classicAddress,
        LimitAmount: {
          currency: IOU_CURRENCY,
          issuer: testContext.carol.classicAddress,
          value: '1000',
        },
      } as never,
      wallet: testContext.alice,
    })
    await Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Payment',
        Account: testContext.carol.classicAddress,
        Destination: testContext.alice.classicAddress,
        Amount: {
          currency: IOU_CURRENCY,
          issuer: testContext.carol.classicAddress,
          value: IOU_AMOUNT,
        },
      } as never,
      wallet: testContext.carol,
    })

    const hook: iHook = {
      CreateCode: readHookBinaryHexFromNS('slot_objects', 'wasm'),
      Flags: HookFlags.hsfOverride,
      HookOn: calculateHookOn(['Invoke']),
      HookNamespace: hexNamespace(namespace),
      HookApiVersion: 0,
    }
    await setHooksV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
      hooks: [{ Hook: hook }],
    } as unknown as SetHookParams)

    // Run each group separately to stay within the instruction limit.
    for (const group of [
      CHK_CHEAP,
      CHK_DEEP,
      CHK_TAKE_FAILURE,
      CHK_CAST,
      CHK_MIDHOP,
      CHK_IOU,
    ]) {
      const params = [
        {
          HookParameter: {
            HookParameterName: convertStringToHex('CHK'),
            HookParameterValue: group.toString(16).padStart(2, '0'),
          },
        },
      ]
      if (group === CHK_IOU) {
        params.push({
          HookParameter: {
            HookParameterName: convertStringToHex('ISS'),
            HookParameterValue: Buffer.from(
              decodeAccountID(testContext.carol.classicAddress),
            )
              .toString('hex')
              .toUpperCase(),
          },
        })
      }

      const response = await Xrpld.submit(testContext.client, {
        tx: {
          TransactionType: 'Invoke',
          Account: testContext.alice.classicAddress,
          Destination: testContext.hook1.classicAddress,
          HookParameters: params,
        } as never,
        wallet: testContext.alice,
      })

      const meta = response.meta as TransactionMetadata
      const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
        testContext.client,
        meta,
      )
      expect(hookExecutions.executions.length).toBe(1)
      const execution = hookExecutions.executions[0]
      expect(execution.HookReturnString).toBe(
        'slot-objects: typed slot layer checks',
      )
      // Hook return codes are hexadecimal RPC values.
      checks |= parseInt(String(execution.HookReturnCode), 16)

      expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
        WORST_CASE_INSTRUCTIONS,
      )
    }
  })

  afterAll(async () => {
    await clearAllHooksV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
    } as unknown as SetHookParams)
    await teardownClient(testContext)
  })

  it('walks the account root with typed reads', () => {
    expect(checks & BIT_ACCOUNT_WALK).toBe(BIT_ACCOUNT_WALK)
  })

  it('round-trips a native amount through as_xfl and back to drops', () => {
    // Native XFL values are denominated in XAH rather than drops.
    expect(checks & BIT_DROPS_ROUNDTRIP).toBe(BIT_DROPS_ROUNDTRIP)
  })

  it('reads a u64 identically through value() and raw bytes', () => {
    expect(checks & BIT_U64_WIRE).toBe(BIT_U64_WIRE)
  })

  it('keeps a child slot readable after its parent is cleared', () => {
    // Child slots must remain valid after their parent is cleared.
    expect(checks & BIT_PARENT_CLEAR).toBe(BIT_PARENT_CLEAR)
  })

  it('accepts a root slot in try_cast::<STObject>', () => {
    expect(checks & BIT_ROOT_CAST).toBe(BIT_ROOT_CAST)
  })

  it('survives 256 successful three-hop walks without exhausting the slots', () => {
    // Recycle slots to remain below the 255-slot limit.
    expect(checks & BIT_DEEP_LOOP).toBe(BIT_DEEP_LOOP)
    expect(checks & BIT_TAKE_LOOP).toBe(BIT_TAKE_LOOP)
  })

  it('leaks nothing when a slot_path! hop fails after two real hops', () => {
    expect(checks & BIT_MIDHOP_LOOP).toBe(BIT_MIDHOP_LOOP)
  })

  it('clears the slot when a take_* read fails', () => {
    expect(checks & BIT_TAKE_FAILURE).toBe(BIT_TAKE_FAILURE)
  })

  it('clears the slot when a try_cast fails', () => {
    expect(checks & BIT_CAST_CLEANUP).toBe(BIT_CAST_CLEANUP)
  })

  it('reads an IOU amount through as_xfl and reports it non-native', () => {
    // Trust-line balances are signed, so the hook compares magnitudes.
    expect(checks & BIT_IOU_XFL).toBe(BIT_IOU_XFL)
  })

  it('passes every check', () => {
    expect(checks).toBe(ALL_CHECKS)
  })
})
