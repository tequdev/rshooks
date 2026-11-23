import { Xrpld } from '@xahau/hooks-toolkit'
import { installHook } from './harness'

// The node has no validator keys (`pubkey_validator` is "none"), so it builds
// no entropy transaction, the Random object never exists, and every draw
// fails with TooLittleEntropy.
describe('rng', () => {
  const getContext = installHook({
    wasmName: 'rng',
    namespace: 'rshooks-e2e-rng',
    hookOn: ['Invoke'],
    clearState: true,
  })

  it('installs and rolls back with NoEntropy (code 1) when the ledger has no entropy', async () => {
    const testContext = getContext()
    const response = Xrpld.submit(testContext.client, {
      tx: {
        TransactionType: 'Invoke',
        Account: testContext.alice.classicAddress,
        Destination: testContext.hook1.classicAddress,
      },
      wallet: testContext.alice,
    })
    await expect(response).rejects.toThrow('1: rng: no entropy')
  })
})
