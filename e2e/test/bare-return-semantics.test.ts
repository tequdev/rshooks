/**
 * Differential test pinning the real on-chain semantics of a bare
 * `return code` from a Hook entry (no `accept!`/`rollback!`) — see
 * `.claude/design/TESTENV_DESIGN.md` §4's `ExitType::Return` discussion.
 * `crates/rshooks-testenv` currently maps a bare return to a
 * **provisional, conservative** non-commit outcome (state snapshot
 * restored, `is_success() == false`) precisely because no live-node
 * evidence pins the real behavior — this test exists to supply that
 * evidence.
 *
 * SKIPPED: no fixture hook exists yet that bare-returns after a state
 * write. Every one of the 15 built examples' `#[hook]` bodies always exits
 * through `accept!`/`rollback!` — none bare-returns — so there is nothing
 * to deploy and invoke here today. Forcing one of the existing examples to
 * bare-return would change its shipped behavior/wasm bytes for a purpose
 * unrelated to its own tutorial, so this is intentionally left as a
 * fixture gap rather than repurposing an example.
 *
 * To un-skip this test:
 * 1. Add a tiny fixture hook whose entry writes a state value and then
 *    bare-returns a nonzero code instead of calling `accept!`/`rollback!`
 *    — either a new numbered example (if that's judged worth a permanent
 *    tutorial slot) or a standalone crate under `e2e/fixtures/` with its
 *    own `rshooks build` step wired into `package.json`'s `pretest` /
 *    `scripts/copy-wasm.mjs` (see that script's `examples` map for the
 *    pattern an additional fixture would follow).
 * 2. Submit a triggering transaction against it, the same way every other
 *    test in this suite does (`Xrpld.submit` +
 *    `ExecutionUtility.getHookExecutionsFromMeta`).
 * 3. Assert the resulting `HookExecution`'s exit type/code, and whether
 *    the state write persisted (`StateUtility.getHookState`).
 * 4. Update `crates/rshooks-testenv/src/exit.rs`'s `ExitType::Return` doc
 *    comment, `book/src/testing/unit-tests.md`'s exit-type table, and
 *    `.claude/design/TESTENV_DESIGN.md` §4 with the pinned result — and
 *    change `TestEnv::invoke`'s mapping in
 *    `crates/rshooks-testenv/src/env.rs` if a bare return turns out to
 *    commit rather than roll back.
 */
describe.skip('bare-return semantics (needs a dedicated fixture hook)', () => {
  it.todo(
    'a bare `return code` after a state write pins its real exit type/code and persistence',
  )
})
