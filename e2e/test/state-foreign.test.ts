import { Xrpld, type XrplIntegrationTestContext } from '@xahau/hooks-toolkit'
import { convertStringToHex, decodeAccountID } from 'xahau'
import { installHook } from './harness'

const namespace = 'rshooks-e2e-state-foreign'

function accountIdHex(classicAddress: string): string {
  return Buffer.from(decodeAccountID(classicAddress)).toString('hex').toUpperCase()
}

async function invoke(testContext: XrplIntegrationTestContext) {
  return Xrpld.submit(testContext.client, {
    tx: {
      TransactionType: 'Invoke',
      Account: testContext.alice.classicAddress,
      Destination: testContext.hook1.classicAddress,
    },
    wallet: testContext.alice,
  })
}

describe('state-foreign: ACCT not configured', () => {
  const getContext = installHook({
    wasmName: 'state_foreign',
    namespace,
    hookOn: ['Invoke'],
  })

  it('rejects when ACCT is not configured', async () => {
    await expect(invoke(getContext())).rejects.toThrow(
      'state-foreign: ACCT parameter not configured',
    )
  })
})

describe('state-foreign: ACCT configured, target has no Hook state at all', () => {
  const getContext = installHook({
    wasmName: 'state_foreign',
    namespace,
    hookOn: ['Invoke'],
    hookParameters: (ctx) => [
      {
        HookParameter: {
          HookParameterName: convertStringToHex('ACCT'),
          HookParameterValue: accountIdHex(ctx.bob.classicAddress),
        },
      },
    ],
  })

  it('rejects with the ReadFailed catch-all, not NotConfiguredOnTarget', async () => {
    await expect(invoke(getContext())).rejects.toThrow(
      'state-foreign: state_foreign read failed',
    )
  })
})
