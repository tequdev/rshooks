import { ExecutionUtility, Xrpld } from '@xahau/hooks-toolkit'
import type { HookEmission, TransactionMetadata } from 'xahau'
import { installHook, readWorstCaseHook } from './harness'

const WORST_CASE_HOOK_INSTRUCTIONS = readWorstCaseHook('10_emit-txn')

describe('emit-txn', () => {
  const getContext = installHook({
    wasmName: 'emit_txn',
    namespace: 'rshooks-e2e-emit-txn',
    hookOn: ['Invoke'],
  })

  it('emits a 1-drop Payment back to the otxn sender, which settles tesSUCCESS with a cbak execution', async () => {
    const testContext = getContext()
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
    expect(Number(execution.HookReturnCode)).toBe(0)
    expect(execution.HookReturnString).toBe('emit-txn: emitted')
    // Hook instruction counts are hexadecimal RPC values.
    expect(parseInt(execution.HookInstructionCount, 16)).toBeLessThanOrEqual(
      WORST_CASE_HOOK_INSTRUCTIONS,
    )

    expect(meta.HookEmissions).toBeDefined()
    const emissions = meta.HookEmissions as HookEmission[]
    expect(emissions.length).toBe(1)
    const emittedTxnID = emissions[0].HookEmission.EmittedTxnID

    // Emissions require a later closed ledger before they can be queried.
    let emittedTxn: any
    for (let attempt = 0; attempt < 4 && !emittedTxn; attempt += 1) {
      await testContext.client.request({ command: 'ledger_accept' } as any)
      try {
        const txResponse = await testContext.client.request({
          command: 'tx',
          transaction: emittedTxnID,
        } as any)
        if ((txResponse as any).result.validated) {
          emittedTxn = txResponse
        }
      } catch {
      }
    }

    if (!emittedTxn) {
      let debugLog = '(docker logs unavailable)'
      try {
        const { execSync } = await import('node:child_process')
        debugLog = execSync('docker logs --tail 200 xahau', {
          encoding: 'utf8',
        })
      } catch (logError) {
        debugLog = `(failed to capture docker logs: ${String(logError)})`
      }
      throw new Error(
        `emitted txn ${emittedTxnID} never validated after 4 ledger_accept calls.\n` +
          `Triggering tx meta: ${JSON.stringify(meta)}\n` +
          `docker logs (tail 200): ${debugLog}`,
      )
    }

    const emittedMeta = emittedTxn.result.meta as TransactionMetadata
    expect(emittedMeta.TransactionResult).toBe('tesSUCCESS')
    expect(emittedTxn.result.Account).toBe(testContext.hook1.classicAddress)
    expect(emittedTxn.result.Destination).toBe(testContext.alice.classicAddress)
    expect(emittedTxn.result.Amount).toBe('1')

    const cbakExecutions = await ExecutionUtility.getHookExecutionsFromMeta(
      testContext.client,
      emittedMeta,
    )
    expect(cbakExecutions.executions.length).toBe(1)
    const cbakExecution = cbakExecutions.executions[0]
    expect(cbakExecution.HookAccount).toBe(testContext.hook1.classicAddress)
    expect(Number(cbakExecution.HookReturnCode)).toBe(0)
    expect(cbakExecution.HookReturnString).toBe('')
    // Use a live sanity bound because callback cost differs from static accounting.
    expect(parseInt(cbakExecution.HookInstructionCount, 16)).toBeLessThanOrEqual(20)
  })
})
