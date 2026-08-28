# `&self` Receiver Design for `#[hooks]` Entries

Status: adopted & implemented (v0.2) — macro support (§3–§6) has landed. The
receiver is now REQUIRED on entry functions (option O3, adopted after the
maintainer authorized breaking changes on the feature branch — see §1). The
canonical-style migration for the book/examples (§9 S1/S2, reflecting the
final O3 decision) is also complete.

Target: rshooks v0.2.x (folded into the feature branch before the v0.2.0 merge — see §8)

Last updated: 2026-08-18

Relates to: [MULTI_HOOK_STRUCT_DESIGN.md](./MULTI_HOOK_STRUCT_DESIGN.md) (r4 §5.5 / §5.7 revision proposal)

## 0. Proposal

In the current v0.2 design, state/param declared in a `#[hooks]` struct are
accessed via a **static with the same name as the struct**:

```rust
#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn deposit() -> i64 {
        let Ok(cfg) = Vault.hook_param.config.get_or_default() else { /* ... */ };
        // ...
    }
}
```

This proposal makes it possible to write the same thing with a `&self` receiver:

```rust
#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn deposit(&self) -> i64 {
        let Ok(cfg) = self.hook_param.config.get_or_default() else { /* ... */ };
        // ...
    }
}
```

## 1. Conclusion (summary)

**Original recommendation: adopt with "optional `&self`"** — the existing
no-receiver form remains legal, with `&self` as the book's canonical style
(this was option O2 in §7).

**Final decision: `&self` is REQUIRED on every entry (O3).** After this
analysis was written, the maintainer authorized breaking changes on the
feature branch, ahead of its merge into v0.2.0. That authorization removed
the constraint that made O2 the recommendation in §7: once a breaking
migration was already on the table, a single uniform style across all code,
examples, and the book outweighed the extra ceremony `&self` adds to the
minimal unit-struct form. §7 records this reasoning in full; §8 records the
resulting timing decision; §9 records the resulting example migration.

- **Measured impact on WCE / size / nesting is zero** (§5). Even across an
  `#[inline(never)]` boundary, the receiver and no-receiver forms produce
  **byte-identical wasm**. There is no cost-based objection.
- DX clearly improves (§4): `self.`-based enumeration/completion of declared
  fields, resilience to struct renames, and natural helper composition.
- The rejection rationale in r4 §5.7 ("`&self` implies an instance that
  doesn't really exist") does not survive re-examination (§2): reading an
  entry as "a method on the sole (static) instance" is, if anything, more
  honest about what the code does than the current "use the struct name as
  a value" form.
- Requiring `&self` on every entry adds ceremony to the declaration-free
  unit-struct minimal form, and free functions outside the `impl` still need
  the static regardless — so mandating `&self` does not, by itself, let the
  no-receiver form be dropped from the language. This was the basis for the
  original O2 recommendation (§7); it was overridden once breaking changes
  were authorized on the feature branch, where the value of one uniform
  style across the whole codebase was judged to outweigh that ceremony cost.

## 2. Re-examining the rationale behind the current design (r4 §5.7)

The reasons r4 §5.7 rejected `self`, and their reassessment:

| r4's rationale | Reassessment |
|---|---|
| "Allowing `&self` creates the false impression that an instance exists" | **An instance already exists.** The moment r4 §5.5 introduced the same-named static (`static Vault: Vault`), "value-style access" via `Vault.deposits` already existed. `&self` just receives that same value as a parameter — it introduces no new misconception. If anything, it replaces the current form's ambiguity ("is the expression `Vault` a type or a value?" — non-obvious to Rust beginners, given the unit-struct value / static value namespaces) with the consistent reading "an entry is a method on the chain instance." |
| "Wrapper generation becomes needlessly complex" | The measured difference is negligible (§6): the wrapper call just changes from `Vault::deposit()` to `Vault::deposit(&Vault)`. Receiver detection is one extra branch in the entry-shape check (the existing `scan` already recognizes receiver shapes) |
| "A diagnostic is needed for `self` misuse (`&mut self`, etc.)" | Needed either way (the current design already has a diagnostic that rejects every kind of `self`). The rejection target just changes from "every kind of `self`" to "any receiver other than `&self`" — the diagnostic implementation cost is about the same |

On the other hand, some of r4's concerns **remain valid**:

- **Encouraging OO-style expectations**: `&self` strengthens the expectation
  that "fields can hold runtime state" or "`&mut self` lets you mutate them."
  → Mitigated by the dedicated diagnostic in §6.2 and an explicit book note
  ("fields are ZST handles; ledger state is only read/written through the
  handle").
- **Reduced consistency from two coexisting forms** (in the optional case).
  → Evaluated in §7.

## 3. Semantics (final adopted design)

### 3.1 Accepted receivers

**On entry functions** (`#[hook(..)]` / `#[cbak(..)]`):

| receiver | treatment |
|---|---|
| none | **ERROR** (dedicated diagnostic: "add `&self` — hook entries require the chain declaration receiver." §6.2) |
| `&self` | **the only legal form** |
| `self` / `mut self` / `&'a self` / type-ascribed `self: T` | ERROR (generic diagnostic: "use `&self`". §6.2) |
| `&mut self` / `&'a mut self` | ERROR (dedicated diagnostic: mutability. §6.2) |

`self` (by value) is technically equivalent since it's a ZST, but to keep the
taught surface to a single form, only `&self` is accepted.

**On helpers** (unattributed associated functions inside an annotated
`impl`, §3.3) the rule is unchanged from the original proposal: no receiver
or `&self` are both legal; `&mut self` (and the other rejected forms above)
remain errors there too.

### 3.2 Wrapper generation

The macro detects whether the entry function has a receiver, and switches
the export wrapper's call accordingly:

```rust
// no receiver (as before)
#[unsafe(export_name = "hook")]
pub extern "C" fn __rshooks_hook_sel_0(_reserved: u32) -> i64 { super::Vault::deposit() }

// &self
#[unsafe(export_name = "hook")]
pub extern "C" fn __rshooks_hook_sel_0(_reserved: u32) -> i64 { super::Vault::deposit(&super::Vault) }
```

`&Vault` is **the same expression for both struct shapes**:

- Named-field struct: `Vault` is the generated static (value namespace)
- Unit struct: `Vault` is the value of the unit constructor

In other words, unit structs never need a generated static just for this
(the existing rule that "unit structs do not generate a static," r4 §5.5,
is unchanged).

### 3.3 `cbak` and helpers

- `#[cbak(i)]` follows the same rule as `#[hook]`: `&self` is the only legal
  entry receiver.
- **Helper functions (no attribute) inside an annotated impl accept `&self`
  (implemented).** Helpers may take no receiver or `&self`; either is legal.
  `&mut self` (and `self` / `mut self` / `&'a self` / type-ascribed
  `self: T`) is rejected for helpers too — it's meaningless for a ZST and
  only invites a mutability misconception. The entry receiver classification
  (§3.1) and the helper classification share the same detection logic
  (`classify_receiver_kinds` / `detect_receiver` in `hooks_impl.rs`).
- Access from free functions and other modules outside the `impl` still uses
  the static (`Vault.deposits`), as before. The static remains part of the
  public interface.

### 3.4 What doesn't change

- The carrier (only entry fn names appear in the JSON; receivers never do)
- rshooks-build (discovery / selective builds / sidecars / templates —
  all unaffected)
- The handshake, marker types, accessor API
- The rest of the diagnostic catalog (index validation, etc.)

## 4. DX analysis

### 4.1 Benefits

1. **Declaration enumeration/completion**: typing `self.` inside an entry
   body lists every state/param the chain declares, in the IDE. "What can
   this hook read?" becomes visible in one keystroke. The static form
   (`Vault.`) also gets completion, but requires recalling and typing the
   struct name, leaving room for spelling drift between entries.
2. **Rename resilience**: renaming the struct no longer requires rewriting
   entry bodies (rust-analyzer's rename does follow static references too,
   but a smaller review diff is still preferable).
3. **More natural Rust**: reading "an entry as a method on the chain
   declaration object" matches what an `impl` block is supposed to mean.
   The "semantic lie" r4 §4.3 admitted to in the struct+impl shape becomes
   a little smaller.
4. **Helper composition**: splitting private methods like
   `self.helper_read()?` becomes natural (previously, helpers also had to
   go through the static or take parameters).
5. **Teaching consistency**: the book's explanation can settle on "`#[hooks]`
   provisions a single (empty) instance of `Vault`, and entries are methods
   on it."

### 4.2 Drawbacks and risks

1. **Encourages OO-style expectations** (§2). Some users will inevitably try
   `&mut self`. → Addressed by the dedicated diagnostic (§6.2) and an
   early-warning note in the book. Since the measured cost is zero, there's
   no "works but is slower" trap.
2. **Two coexisting forms** (in the optional case): a codebase could mix
   `self.x` and `Vault.x` in the same file. → the book and examples
   standardize on `&self`, with the no-receiver form positioned as "the
   shape of a minimal hook with no declarations." No lint-level enforcement.
3. **Diagnostic/fixture update cost**: rewording the existing "Hook
   entrypoints are stateless associated functions" diagnostic, plus trybuild
   updates (§6.3). Small.
4. **No added confusion around the `Self` type**: `Self::helper()` already
   works today. Whether a receiver is present doesn't affect `Self`'s
   visibility.

## 5. Measured impact on WCE / size / nesting

Using the actual rshooks-build pipeline (same profile as the examples:
discovery + selective build + clean/flatten/unnest/guard/validate), the same
logic (about 6 typed-accessor calls: one param read plus a state
read/modify/write pair) was built and compared across six forms.

| Form | WCE (hook) | Size (bytes) | Max nesting | Notes |
|---|---:|---:|---:|---|
| V0: current form (inline in the entry, via static) | 630 | 1562 | 4 | baseline |
| V1: delegated to a `&self` method (no inline directive) | 630 | 1561 | 4 | Effectively identical to V0 (the only difference is 1 byte of rodata, from symbol-name-driven data layout, not a code-path difference) |
| V0n: no-receiver helper, `#[inline(never)]` | 637 | 1574 | 4 | |
| V1n: `&self` helper, `#[inline(never)]` | 637 | 1574 | 4 | **byte-identical to V0n** |
| V2n: split into 3, `#[inline(never)]` (no receiver) | 684 | 1665 | 5 | |
| V2: split into 3, `#[inline(never)]` (`&self`) | 684 | 1665 | 5 | **byte-identical to V2n** |

Takeaways:

1. **`&self` itself costs nothing.** When LLVM is free to inline (V1), it
   disappears entirely; even across an `#[inline(never)]` boundary (V1n/V2)
   it's byte-identical to the no-receiver version. References to a ZST
   vanish under optimization and never show up as argument spill in the
   flatten pass.
2. Cost only comes from the **number of `#[inline(never)]` boundaries**
   (V0→V0n: +7 WCE; 1 boundary → 3 boundaries: +47 WCE, nesting 4→5), which
   is unrelated to receiver presence. This is a known function-boundary
   cost that this proposal neither increases nor decreases.
3. It is therefore accurate to state in the book that "`&self` is a
   zero-cost abstraction" (backed by measurement).

(Aside: the "nesting explosion from a high density of typed accessors" issue
in `80_governance` (a build-budget finding) is a function of call
**density**, independent of receiver form. Adopting or not adopting `&self`
neither improves nor worsens that issue.)

## 6. Implementation impact

### 6.1 Change list

| Area | Change | Size |
|---|---|---|
| `hooks_impl.rs` entry-shape check | Receiver detection changes from "reject" to "none / `&self`, a two-valued check." Branches the wrapper call expression accordingly | Small |
| `hooks_impl.rs` helper classification | Let unattributed functions accept `&self` (`&mut self` still rejected) | Small |
| Diagnostics | Replaced with the wording in §6.2 | Small |
| trybuild fixtures | §6.3 | Small |
| book / examples | Canonical style switched to `&self` (bundled with the adoption decision) | Medium (mechanical) |
| decl / build / carrier | **No change** | — |

### 6.2 Diagnostics (draft)

- No receiver on an entry: "add `&self` — hook entries require the chain
  declaration receiver (it is zero-sized)"
- `self` / `mut self` / `&'a self` / type-ascribed: "use `&self` — hook
  entrypoints receive the chain declaration by shared reference (it is
  zero-sized)"
- `&mut self` / `&'a mut self`: "chain handles are zero-sized and immutable;
  ledger state is accessed through the handles, not by mutating the struct
  — use `&self`"
- These replace the previous "stateless associated functions" wording once
  finalized.

### 6.3 trybuild

- Existing fail case `hooks_self_receiver.rs` (rejected every kind of
  `self`) is split: `&self` moves to the pass side as the required form; the
  fail side is split into no-receiver-on-entry, `&mut self` / `mut self`,
  and by-value `self`.
- New pass cases: `&self` entries plus `&self` helpers, `&self` on `cbak`,
  and helpers mixing no-receiver and `&self` in the same impl (pinning down
  that both helper forms remain legal even though entries require `&self`).

### 6.4 Compatibility

Making `&self` mandatory (the final decision, O3) is a **breaking change**
relative to both the no-receiver form and the optional form (O2) this
document originally recommended: every existing entry needs a `&self`
parameter added. This is acceptable because the change was folded into the
v0.2.0 feature branch **before it merged** (§8) — there is no released
version with the optional or no-receiver entry shape that this breaks, so no
separate semver-breaking release is required. Had this been made mandatory
after v0.2.0 had already merged, it would have been a second breaking
change.

## 7. Option comparison

| | O1: status quo | O2: optional `&self` (original recommendation) | **O3: `&self` mandatory (final decision)** |
|---|---|---|---|
| WCE | — | zero impact (measured) | zero impact (measured) |
| DX | depends on spelling the static name correctly | `self.` completion, a uniform reading. Coexisting forms is the only weakness | one form only — most consistent |
| Breakage | none | **none** (additive) | every entry rewritten (essentially free before PR #53 merges; breaking after) |
| Minimal form (unit struct) | `fn main() -> i64` | unchanged | `fn main(&self) -> i64` — adds ceremony to a hook with no declarations |
| Access from outside impl | static | static (coexists) | static (still required — a "self-only world" isn't achievable) |
| Teaching | must explain "struct name = value" | "entries are methods" + "static from the outside" | same, but no need to explain the no-receiver form |

**Final decision: O3.** The two reasons that originally ruled out O3 remain
technically valid — they are exactly why this document's original
recommendation was O2:

(a) since static access remains necessary from outside the `impl` (free
functions, other modules), "access is always through `self`" was never
going to be a complete story regardless of this decision — statics stay
public (§3.3);

(b) writing `&self` on the declaration-free unit-struct minimal form (the
book's first example) forces beginners to be told upfront "what's in this
`self`? (answer: nothing)," which is a worse teaching order.

What changed the outcome was the authorization for breaking changes on the
feature branch (§1): once a breaking migration was already acceptable, the
calculus shifted from "minimize ceremony in the minimal form" to "minimize
the number of styles a reader has to learn." A single uniform entry style —
always `&self` — across every code sample, example crate, and book page was
judged worth the ceremony cost in (b). Reason (a) is unaffected by this
decision and still holds exactly as stated: O3 does not make "access is
always through `self`" complete, it only makes the *entry* function shape
uniform.

## 8. Adoption timing

- **If the original recommendation (O2) had been adopted**: being additive,
  it could ship decoupled from PR #53 — as an independent, low-risk small PR
  after merge (macro + diagnostics + fixtures + book canonical-style switch
  + `&self`-ifying the examples).
- **Since O3 (mandatory) was chosen instead**: it needed to land **before**
  PR #53 merged (landing it afterward would have made it a second breaking
  change). This required rewriting all 16 example crates as part of the same
  branch.

**Resolved**: `&self` was made mandatory (O3) and folded into the feature
branch, completed **before** that branch's merge into v0.2.0.

## 9. Open questions (→ decisions)

| ID | Question | Recommendation | Status |
|---|---|---|---|
| S1 | Migrate all examples to the `&self` canonical style at once, or only new pages | Migrate everything at once (don't let the book and examples diverge) | **Superseded by the O3 mandate (§1/§7) and then decided/implemented on that basis**: since `&self` is now required on every entry, the original question is moot — every entry across all examples (01–15, and 80_governance's `reward` and `govern`) now takes `&self`, whether or not it reads a declared field; entries that don't use the receiver simply carry an unused `&self` parameter. WCE/size/nesting are byte-identical before/after for entries that already read declared fields (measured on 02/12/80_governance); the newly added unused `&self` parameter on declaration-free entries also measured byte-identical (a ZST parameter never spills). |
| S2 | How to present direct `Vault.` static access in the book | Explain it in one place, as "the way to access from outside the impl" | **Decided & implemented**: consolidated under "The struct has no runtime instance — but entries may borrow it" in `concepts/anatomy.md`, cross-referenced from `data/state.md` / `data/parameters.md` / `reference/macros.md`. This remains accurate under O3: the static is still the only way to reach declared fields from outside the `impl` (§3.3, §7). |
| S3 | Whether to allow `&mut self` on helpers in the future (harmless in principle, since it's a ZST) | Keep rejecting it (an educational line against the mutability misconception) | Unchanged (out of scope for this migration) |
| S4 | Whether to add a clippy-style lint (nudging no-receiver entries) as a build-side info diagnostic | Don't (both forms were equally valid; a noisy lint hurts DX) | Unchanged (out of scope for this migration) |

## 10. References

- [MULTI_HOOK_STRUCT_DESIGN.md](./MULTI_HOOK_STRUCT_DESIGN.md) r4 §4.3 / §5.5 / §5.7 (the sections this proposal revises)
- Measurement probe: WCE/size/nesting comparison across the same logic in
  six forms (the table in §5; measured against a real build pipeline using
  scratch crates, 2026-08-18)
- `crates/rshooks/tests/ui/pass/hooks_impl_qualified_helpers.rs` (current
  helper pass-through spec)
