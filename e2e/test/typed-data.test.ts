import {
  ExecutionUtility,
  StateUtility,
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

const namespace = 'rshooks-e2e-typed-data'
const WORST_CASE_INSTRUCTIONS = 439

const ACTION_DEPOSIT = 1
const ACTION_WITHDRAW = 2

const DEPOSIT_TAG = 1

const MIN_DROPS = 5_000_000n // 5 XAH
// Keep the lock window above the ledger advance between submissions.
const LOCK_LEDGERS = 30

function u64LEHex(value: bigint): string {
  const buf = Buffer.alloc(8)
  buf.writeBigUInt64LE(value)
  return buf.toString('hex').toUpperCase()
}

function u32LEHex(value: number): string {
  const buf = Buffer.alloc(4)
  buf.writeUInt32LE(value)
  return buf.toString('hex').toUpperCase()
}

function cfgHex(minAmount: bigint, lockLedgers: number): string {
  return u64LEHex(minAmount) + u32LEHex(lockLedgers)
}

function insHex(action: number, amount: bigint): string {
  const actionByte = Buffer.from([action]).toString('hex').toUpperCase()
  return actionByte + u64LEHex(amount)
}

// The host left-pads this 21-byte state key to 32 bytes.
function depositStateKeyHex(address: string): string {
  const owner = Buffer.from(decodeAccountID(address)).toString('hex')
  const tag = DEPOSIT_TAG.toString(16).padStart(2, '0')
  return (tag + owner).toUpperCase().padStart(64, '0')
}

// A missing namespace after its final entry is deleted means no deposit exists.
async function depositEntryExists(
  testContext: XrplIntegrationTestContext,
  address: string,
): Promise<boolean> {
  const wanted = depositStateKeyHex(address)
  let entries
  try {
    entries = await StateUtility.getHookStateDir(
      testContext.client,
      testContext.hook1.classicAddress,
      hexNamespace(namespace),
    )
  } catch (error) {
    if (String((error as Error)?.message ?? '').includes('Namespace not found')) {
      return false
    }
    throw error
  }
  return entries.some((e) => e.HookStateKey.toUpperCase() === wanted)
}

function hookParam(name: string, valueHex: string) {
  return {
    HookParameter: {
      HookParameterName: convertStringToHex(name),
      HookParameterValue: valueHex,
    },
  }
}

async function invoke(
  testContext: XrplIntegrationTestContext,
  sender: XrplIntegrationTestContext['alice'],
  insValueHex: string,
) {
  return Xrpld.submit(testContext.client, {
    tx: {
      TransactionType: 'Invoke',
      Account: sender.classicAddress,
      Destination: testContext.hook1.classicAddress,
      HookParameters: [hookParam('INS', insValueHex)],
    } as any,
    wallet: sender,
  })
}

describe('typed-data', () => {
  let testContext: XrplIntegrationTestContext

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)

    const hook: iHook = {
      CreateCode: readHookBinaryHexFromNS('typed_data', 'wasm'),
      Flags: HookFlags.hsfOverride,
      HookOn: calculateHookOn(['Invoke']),
      HookNamespace: hexNamespace(namespace),
      HookApiVersion: 0,
      HookParameters: [hookParam('CFG', cfgHex(MIN_DROPS, LOCK_LEDGERS))],
    }
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

  it('rejects an Invoke with no INS parameter', async () => {
    const response = Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Invoke',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
      },
      wallet: testContext.alice,
    })
    await expect(response).rejects.toThrow(
      'typed-data: INS parameter missing or malformed',
    )
  })

  it('rejects a withdraw when the account has no outstanding deposit', async () => {
    // bob has never deposited, so `DepositState { tag: 1, owner: bob }` has no
    // state entry - `state_get` returns `None`, decoded as `EMPTY_DEPOSIT`
    // (`flags == 0`).
    const response = invoke(
      testContext,
      testContext.bob,
      insHex(ACTION_WITHDRAW, 0n),
    )
    await expect(response).rejects.toThrow('typed-data: nothing to withdraw')
  })

  it('rejects a deposit below the configured minimum', async () => {
    const response = invoke(
      testContext,
      testContext.alice,
      insHex(ACTION_DEPOSIT, MIN_DROPS - 1n),
    )
    await expect(response).rejects.toThrow(
      'typed-data: deposit below configured minimum',
    )
  })

  it('accepts a deposit at the configured minimum', async () => {
    const response = await invoke(
      testContext,
      testContext.alice,
      insHex(ACTION_DEPOSIT, MIN_DROPS),
    )

    const meta = response.meta as TransactionMetadata
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
      testContext.client,
      meta,
    )
    expect(hookExecutions.executions.length).toBe(1)
    const execution = hookExecutions.executions[0]
    // `accept!(b"typed-data: ok", next.amount as i64)` - the resulting
    // balance (here, exactly `MIN_DROPS`, alice's first deposit) becomes
    // the `HookReturnCode`, a hex string over RPC (see hook-params.test.ts
    // for the same convention).
    expect(BigInt(`0x${execution.HookReturnCode}`)).toBe(MIN_DROPS)
    expect(execution.HookReturnString).toBe('typed-data: ok')
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_INSTRUCTIONS,
    )
  })

  it('stores the deposit as a hook state entry', async () => {
    // The positive half of the delete assertion below: without this, an
    // "entry is absent after withdraw" test would also pass if the key were
    // computed wrongly and never matched anything in the first place.
    await expect(
      depositEntryExists(testContext, testContext.alice.classicAddress),
    ).resolves.toBe(true)
  })

  it('rejects a withdraw before the lock window elapses', async () => {
    const response = invoke(
      testContext,
      testContext.alice,
      insHex(ACTION_WITHDRAW, 0n),
    )
    await expect(response).rejects.toThrow('typed-data: deposit still locked')
  })

  it('accepts a withdraw once the lock window has elapsed', async () => {
    // `LOCK_LEDGERS = 30`: force-advance the ledger well past the
    // deposit's `deadline = deposit_ledger_seq + 30` via the standalone
    // node's `ledger_accept` admin RPC - the same mechanism
    // emit-txn.test.ts uses to satisfy an emitted transaction's
    // `FirstLedgerSequence`.
    for (let i = 0; i < 35; i += 1) {
      await testContext.client.request({ command: 'ledger_accept' } as any)
    }

    const response = await invoke(
      testContext,
      testContext.alice,
      insHex(ACTION_WITHDRAW, 0n),
    )

    const meta = response.meta as TransactionMetadata
    const hookExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
      testContext.client,
      meta,
    )
    expect(hookExecutions.executions.length).toBe(1)
    const execution = hookExecutions.executions[0]
    // A withdrawal always empties the whole balance, so the accepted
    // return code is the resulting balance: zero.
    expect(Number(execution.HookReturnCode)).toBe(0)
    expect(execution.HookReturnString).toBe('typed-data: ok')
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_INSTRUCTIONS,
    )
  })

  it('deletes the state entry on a full withdrawal', async () => {
    // The live proof of `rshooks::state::state_delete` /
    // `key.delete_state()`: the entry that the deposit test just observed
    // is now **gone** from the namespace directory, not present-and-zeroed.
    // Nothing on a host build can demonstrate this - every Hook API call
    // there is a stub that returns `NotImplemented` without touching state.
    await expect(
      depositEntryExists(testContext, testContext.alice.classicAddress),
    ).resolves.toBe(false)
  })

  it('rejects a second withdraw now that the deposit is gone', async () => {
    const response = invoke(
      testContext,
      testContext.alice,
      insHex(ACTION_WITHDRAW, 0n),
    )
    await expect(response).rejects.toThrow('typed-data: nothing to withdraw')
  })

  it('rejects an unknown INS action', async () => {
    const response = invoke(
      testContext,
      testContext.bob,
      insHex(99, 0n),
    )
    await expect(response).rejects.toThrow('typed-data: unknown INS action')
  })

  describe('deposit pause switch (composite AdminName parameter)', () => {
    // `AdminName { section: 0, field: 0 }` - 2 bytes, back-to-back, no
    // padding (README's "Hex encoding" section under "Composite
    // (struct-shaped) parameter names").
    const ADMIN_NAME_HEX = '0000'

    beforeAll(async () => {
      // Re-install the same hook with an additional composite `AdminName`
      // Hook parameter, `PauseSwitch { paused: 1 }` (hex `01`) - pauses new
      // deposits without touching any existing `DepositValue` state (the
      // HookNamespace, and so every account's state entry, is unchanged by
      // a SetHook that only replaces the hook definition/parameters).
      const hook: iHook = {
        CreateCode: readHookBinaryHexFromNS('typed_data', 'wasm'),
        Flags: HookFlags.hsfOverride,
        HookOn: calculateHookOn(['Invoke']),
        HookNamespace: hexNamespace(namespace),
        HookApiVersion: 0,
        HookParameters: [
          hookParam('CFG', cfgHex(MIN_DROPS, LOCK_LEDGERS)),
          {
            HookParameter: {
              HookParameterName: ADMIN_NAME_HEX,
              HookParameterValue: '01',
            },
          },
        ],
      }
      await setHooksV3({
        client: testContext.client,
        seed: testContext.hook1.seed,
        hooks: [{ Hook: hook }],
      } as unknown as SetHookParams)
    })

    afterAll(async () => {
      // Restore the unpaused hook (no `AdminName` parameter at all - absent
      // is treated the same as `paused: 0`, per `deposits_paused`'s doc
      // comment) so nothing after this block observes deposits paused.
      const hook: iHook = {
        CreateCode: readHookBinaryHexFromNS('typed_data', 'wasm'),
        Flags: HookFlags.hsfOverride,
        HookOn: calculateHookOn(['Invoke']),
        HookNamespace: hexNamespace(namespace),
        HookApiVersion: 0,
        HookParameters: [hookParam('CFG', cfgHex(MIN_DROPS, LOCK_LEDGERS))],
      }
      await setHooksV3({
        client: testContext.client,
        seed: testContext.hook1.seed,
        hooks: [{ Hook: hook }],
      } as unknown as SetHookParams)
    })

    it('rejects a deposit while the AdminName pause switch is set', async () => {
      // bob has no outstanding deposit at this point in the suite (his two
      // earlier invokes both rolled back, so no state was ever written) -
      // a deposit here exercises the pause check regardless.
      const response = invoke(
        testContext,
        testContext.bob,
        insHex(ACTION_DEPOSIT, MIN_DROPS),
      )
      await expect(response).rejects.toThrow(
        'typed-data: deposits are currently paused',
      )
    })

    it('still allows a withdraw while paused (withdrawals are never paused)', async () => {
      // bob still has no outstanding deposit, so this hits the *other*
      // rollback path (`NothingToWithdraw`) rather than succeeding - but
      // reaching that check at all (instead of `DepositsPaused`) proves
      // `deposits_paused()` is only ever consulted on the deposit branch,
      // exactly as documented.
      const response = invoke(
        testContext,
        testContext.bob,
        insHex(ACTION_WITHDRAW, 0n),
      )
      await expect(response).rejects.toThrow('typed-data: nothing to withdraw')
    })
  })
})
