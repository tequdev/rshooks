import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    globals: true,
    watch: false,
    // Tests share standalone ledger state.
    fileParallelism: false,
    sequence: {
      concurrent: false,
      // Suites' own beforeAll hooks (e.g. slot-objects, govern) read the
      // testContext a harness beforeAll registered first - hooks must run
      // in registration order, not vitest's default parallel fan-out.
      hooks: 'list',
    },
    // SetHook and ledger operations can take longer than the default timeout.
    testTimeout: 60_000,
    hookTimeout: 60_000,
  },
})
