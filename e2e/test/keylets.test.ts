import { createHash } from 'node:crypto'
import {
  ExecutionUtility,
  StateUtility,
  Xrpld,
  clearAllHooksV3,
  clearHookStateV3,
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
  decodeAccountID,
  hashes,
  type TransactionMetadata,
} from 'xahau'
import { HookFlags } from 'xahau/dist/npm/models/common/xahau'
import { hashCron } from 'xahau/dist/npm/utils/hashes'

const namespace = 'rshooks-e2e-keylets'
const WORST_CASE_INSTRUCTIONS = 4150

const SPACE = {
  account: 'a',
  signerList: 'S',
  rippleState: 'r',
  offer: 'o',
  escrow: 'u',
  paychan: 'x',
  check: 'C',
  ticket: 'T',
  depositPreauth: 'p',
  ownerDir: 'O',
  amendment: 'f',
  feeSettings: 'e',
  cron: 'L',
} as const

const TEST_HASH_HEX = 'AB'.repeat(32)

const OFFER_SEQ = 1
const ESCROW_SEQ = 2
const CHECK_SEQ = 3
const PAYCHAN_SEQ = 5
const CRON_START_TIME = 1_700_000_000

// State-key discriminants are left-padded by the host.
function keyletStateKeyHex(discriminant: number): string {
  return discriminant.toString(16).padStart(2, '0').toUpperCase().padStart(64, '0')
}

function accountHex(address: string): string {
  return Buffer.from(decodeAccountID(address)).toString('hex')
}

function u32Hex(value: number): string {
  return value.toString(16).padStart(8, '0')
}

// Some keylet type codes differ from their hash-space prefixes.
function spacePrefixHex(spaceChar: string): string {
  return spaceChar.charCodeAt(0).toString(16).padStart(4, '0')
}

function sha512Half(hex: string): string {
  return createHash('sha512').update(Buffer.from(hex, 'hex')).digest('hex').slice(0, 64)
}

function keyletHex(spaceChar: string, indexHex: string): string {
  return (spacePrefixHex(spaceChar) + indexHex).toUpperCase()
}

function typedKeyletHex(typeCodeHex: string, indexHex: string): string {
  return (typeCodeHex + indexHex).toUpperCase()
}

function expectedAccount(owner: string): string {
  return keyletHex(SPACE.account, hashes.hashAccountRoot(owner))
}
function expectedSigners(owner: string): string {
  return keyletHex(SPACE.signerList, hashes.hashSignerListId(owner))
}
function expectedLine(a: string, b: string, currency: string): string {
  return keyletHex(SPACE.rippleState, hashes.hashTrustline(a, b, currency))
}
function expectedOffer(owner: string, seq: number): string {
  return keyletHex(SPACE.offer, hashes.hashOfferId(owner, seq))
}
function expectedEscrow(owner: string, seq: number): string {
  return keyletHex(SPACE.escrow, hashes.hashEscrow(owner, seq))
}
function expectedPaychan(src: string, dst: string, seq: number): string {
  return keyletHex(SPACE.paychan, hashes.hashPaymentChannel(src, dst, seq))
}
function expectedCron(owner: string, time: number): string {
  return typedKeyletHex('0041', hashCron(owner, time))
}
function expectedCheck(owner: string, seq: number): string {
  return keyletHex(
    SPACE.check,
    sha512Half(spacePrefixHex(SPACE.check) + accountHex(owner) + u32Hex(seq)),
  )
}
function expectedDepositPreauth(owner: string, authorized: string): string {
  return keyletHex(
    SPACE.depositPreauth,
    sha512Half(
      spacePrefixHex(SPACE.depositPreauth) + accountHex(owner) + accountHex(authorized),
    ),
  )
}
function expectedOwnerDir(owner: string): string {
  return typedKeyletHex(
    '0064',
    sha512Half(spacePrefixHex(SPACE.ownerDir) + accountHex(owner)),
  )
}
function expectedAmendments(): string {
  return keyletHex(SPACE.amendment, sha512Half(spacePrefixHex(SPACE.amendment)))
}
function expectedFees(): string {
  return typedKeyletHex('0073', sha512Half(spacePrefixHex(SPACE.feeSettings)))
}
function expectedUnchecked(hashHex: string): string {
  return ('0000' + hashHex).toUpperCase()
}

describe('keylets', () => {
  let testContext: XrplIntegrationTestContext
  const hookNamespace = hexNamespace(namespace)

  beforeAll(async () => {
    testContext = await setupClient(serverUrl)

    const hook: iHook = {
      CreateCode: readHookBinaryHexFromNS('keylets', 'wasm'),
      Flags: HookFlags.hsfOverride,
      HookOn: calculateHookOn(['Invoke']),
      HookNamespace: hookNamespace,
      HookApiVersion: 0,
    }
    await setHooksV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
      hooks: [{ Hook: hook }],
    } as unknown as SetHookParams)
  })

  afterAll(async () => {
    const clearStateHook: iHook = {
      Flags: HookFlags.hsfNSDelete,
      HookNamespace: hookNamespace,
    }
    await clearHookStateV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
      hooks: [{ Hook: clearStateHook }],
    } as unknown as SetHookParams)
    await clearAllHooksV3({
      client: testContext.client,
      seed: testContext.hook1.seed,
    } as unknown as SetHookParams)
    await teardownClient(testContext)
  })

  const owner = () => testContext.alice.classicAddress
  const dest = () => testContext.hook1.classicAddress

  it('accepts the Invoke and writes every keylet except Ticket', async () => {
    const response = await Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Invoke',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
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
    expect(execution.HookReturnString).toBe('keylets: ok')
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_INSTRUCTIONS,
    )
  })

  async function readKeylet(discriminant: number): Promise<string> {
    const entry = await StateUtility.getHookState(
      testContext.client,
      testContext.hook1.classicAddress,
      keyletStateKeyHex(discriminant),
      hookNamespace,
    )
    return entry.HookStateData.toUpperCase()
  }

  function wellFormed(actual: string) {
    expect(Buffer.from(actual, 'hex').length).toBe(34)
    expect(actual).not.toBe('00'.repeat(34).toUpperCase())
  }


  it('Account (KEYLET_ACCOUNT, discriminant 2)', async () => {
    expect(await readKeylet(2)).toBe(expectedAccount(owner()))
  })

  it('Signers (KEYLET_SIGNERS, discriminant 13)', async () => {
    expect(await readKeylet(13)).toBe(expectedSigners(owner()))
  })

  it('Line (KEYLET_LINE, discriminant 8)', async () => {
    expect(await readKeylet(8)).toBe(expectedLine(owner(), dest(), 'USD'))
  })

  it('Offer (KEYLET_OFFER, discriminant 9)', async () => {
    expect(await readKeylet(9)).toBe(expectedOffer(owner(), OFFER_SEQ))
  })

  it('Escrow (KEYLET_ESCROW, discriminant 19)', async () => {
    expect(await readKeylet(19)).toBe(expectedEscrow(owner(), ESCROW_SEQ))
  })

  it('Paychan (KEYLET_PAYCHAN, discriminant 20)', async () => {
    expect(await readKeylet(20)).toBe(
      expectedPaychan(owner(), dest(), PAYCHAN_SEQ),
    )
  })

  it('Cron (KEYLET_CRON, discriminant 25)', async () => {
    expect(await readKeylet(25)).toBe(expectedCron(owner(), CRON_START_TIME))
  })

  it('Check (KEYLET_CHECK, discriminant 14)', async () => {
    expect(await readKeylet(14)).toBe(expectedCheck(owner(), CHECK_SEQ))
  })


  it('DepositPreauth (KEYLET_DEPOSIT_PREAUTH, discriminant 15)', async () => {
    expect(await readKeylet(15)).toBe(expectedDepositPreauth(owner(), dest()))
  })

  it('OwnerDir (KEYLET_OWNER_DIR, discriminant 17)', async () => {
    expect(await readKeylet(17)).toBe(expectedOwnerDir(owner()))
  })

  it('Amendments (KEYLET_AMENDMENTS, discriminant 3)', async () => {
    expect(await readKeylet(3)).toBe(expectedAmendments())
  })

  it('Fees (KEYLET_FEES, discriminant 6)', async () => {
    expect(await readKeylet(6)).toBe(expectedFees())
  })

  it('Unchecked (KEYLET_UNCHECKED, discriminant 16)', async () => {
    expect(await readKeylet(16)).toBe(expectedUnchecked(TEST_HASH_HEX))
  })


  it.each([
    ['Hook', 0],
    ['HookState', 1],
    ['Child', 4],
    ['Skip', 5],
    ['NegativeUnl', 7],
    ['Quality', 10],
    ['EmittedDir', 11],
    ['Page', 18],
    ['Emitted', 21],
    ['NftOffer', 22],
    ['HookDefinition', 23],
    ['HookStateDir', 24],
  ])('%s is a well-formed 34-byte keylet', async (_name, discriminant) => {
    wellFormed(await readKeylet(discriminant as number))
  })
})
