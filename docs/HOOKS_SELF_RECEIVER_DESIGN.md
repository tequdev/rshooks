# `&self` Receiver Design for `#[hooks]` Entries

Status: current spec. [MULTI_HOOK_STRUCT_DESIGN.md](./MULTI_HOOK_STRUCT_DESIGN.md)
§5.7 has the one-paragraph summary; this file keeps the receiver
classification and diagnostic wording the macro implementation
(`crates/rshooks-macros/src/hooks_impl.rs`) and its UI test fixtures
(`crates/rshooks/tests/ui/{pass,fail}/hooks_*self*.rs`) cite by section
number.

## 3. Semantics

### 3.1 Accepted receivers

**On entry functions** (`#[hook(..)]` / `#[cbak(..)]`):

| receiver | treatment |
|---|---|
| none | **ERROR** (dedicated diagnostic: "hook entry functions take `&self` — the chain declaration is passed by shared reference (it is zero-sized)". §6.2) |
| `&self` | **the only legal form** |
| `self` / `mut self` / `&'a self` / type-ascribed `self: T` | ERROR (generic diagnostic: "use `&self`". §6.2) |
| `&mut self` / `&'a mut self` | ERROR (dedicated diagnostic: mutability. §6.2) |

`self` (by value) is technically equivalent since it's a ZST, but to keep the
taught surface to a single form, only `&self` is accepted.

**On helpers** (unattributed associated functions inside an annotated
`impl`, §3.3): no receiver or `&self` are both legal; `&mut self` (and the
other rejected forms above) remain errors there too.

### 3.2 Wrapper generation

`&{struct_name}` is the entry call's receiver argument, and is the same
expression for both struct shapes:

- Named-field struct: `Vault` is the generated same-named static (value
  namespace).
- Unit struct: `Vault` is the value of the unit constructor.

```rust
#[unsafe(export_name = "hook")]
pub extern "C" fn __rshooks_hook_sel_0(_reserved: u32) -> i64 { super::Vault::deposit(&super::Vault) }
```

Unit structs never need a generated static just for this.

### 3.3 `cbak` and helpers

- `#[cbak(i)]` follows the same rule as `#[hook]`: `&self` is the only
  legal entry receiver.
- Helper functions (no attribute) inside an annotated impl accept no
  receiver or `&self`; `&mut self` (and `self` / `mut self` / `&'a self` /
  type-ascribed `self: T`) is rejected for helpers too — it's meaningless
  for a ZST and only invites a mutability misconception. The entry receiver
  classification (§3.1) and the helper classification share the same
  detection logic (`classify_receiver_kinds` / `detect_receiver` in
  `hooks_impl.rs`).
- `#[cfg]`/`#[cfg_attr]` on a receiver is rejected: the receiver's class is
  decided once at macro-expansion time, before `cfg` resolves, so a
  conditional shape would diverge from what actually compiles.
- Access from free functions and other modules outside the `impl` still
  uses the static (`Vault.deposits`). The static remains part of the
  public interface.

## 6. Diagnostics

### 6.2 Wording

- No receiver on an entry: "hook entry functions take `&self` — the chain
  declaration is passed by shared reference (it is zero-sized)"
- `self` / `mut self` / `&'a self` / type-ascribed: "use `&self` — hook
  entrypoints receive the chain declaration by shared reference (it is
  zero-sized)"
- `&mut self` / `&'a mut self`: "chain handles are zero-sized and immutable;
  ledger state is accessed through the handles, not by mutating the struct
  — use `&self`"
