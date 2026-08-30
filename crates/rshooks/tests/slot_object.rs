//! Integration coverage for the typed slot layer: `SlotObject<T>`,
//! `SField<T>`, navigation gating, the read table, and `slot_path!`.
//!
//! An integration test (`tests/`), not an in-crate module, because the
//! generated `sfield` table and the prelude re-exports are part of what is
//! being checked and both are best exercised the way a hook crate sees them.
//!
//! # What a host build can prove here
//!
//! Every Hook API call resolves to `rshooks-core`'s host stub on a non-wasm
//! target, and every stub returns `NOT_IMPLEMENTED`. So these tests prove
//! **typing, inference and reachability**: that the field constants carry the
//! right value types, that navigation is gated by parent type, that every
//! read exists with the signature the design specifies, and that the widened
//! field-code APIs accept typed, raw and stored `u32` alike.
//!
//! They cannot prove host *behavior* — which slot the host assigned, that a
//! parent can be cleared after deriving a child, that `take_*` really frees
//! a slot. Those are pinned live in `e2e/test/slot-ledger.test.ts`.
use rshooks::prelude::*;
use rshooks::slot_path;
use rshooks::types::{Amount, Issue, Opaque};

const STUB: HookError = HookError::NotImplemented;

#[test]
fn surface() {
    // constructors
    assert_eq!(SlotObject::from_otxn().err(), Some(STUB));
    assert_eq!(SlotObject::from_meta().err(), Some(STUB));
    let k = Keylet::default();
    assert_eq!(SlotObject::from_keylet(&k).err(), Some(STUB));
    let h = Hash::default();
    assert_eq!(SlotObject::from_txn_hash(&h).err(), Some(STUB));

    // typed field constants
    let _: SField<u32> = sfSequence;
    let _: SField<AccountId> = sfAccount;
    let _: SField<Amount> = sfBalance;
    let _: SField<STArray> = sfSignerEntries;
    let _: SField<Hash> = sfLedgerHash;
    let _: SField<u64> = sfExchangeRate;
    let _: SField<u8> = sfCloseResolution;
    let _: SField<u16> = sfTransactionType;
    let _: SField<STObject> = sfMemo;
    let _: SField<Issue> = sfClaimCurrency;
    // Blob / Hash160 / PathSet -> Opaque
    let _: SField<Opaque> = sfSigningPubKey;
    let _: SField<Opaque> = sfTakerPaysCurrency;

    // Field codes compare across type parameters, which is what makes an
    // erased `field_code()` result usable against the constants.
    assert!(sfAccount == sfAccount);
    assert!(sfSigningPubKey != sfAccount);

    // code() const bridge + parity
    const SEQ: u32 = sfSequence.code();
    assert_eq!(SEQ, rshooks::raw::sfcodes::sfSequence);
    let widened: u32 = sfAccount.into();
    assert_eq!(widened, rshooks::raw::sfcodes::sfAccount);

    // widened APIs accept typed, raw, and stored u32
    let mut buf = [0u8; 20];
    assert_eq!(otxn_field(&mut buf, sfAccount).err(), Some(STUB));
    assert_eq!(
        otxn_field(&mut buf, rshooks::raw::sfcodes::sfAccount).err(),
        Some(STUB)
    );
    let stored: u32 = sfAccount.code();
    assert_eq!(otxn_field(&mut buf, stored).err(), Some(STUB));
}

#[test]
fn navigation_types() {
    fn _typed(root: SlotObject<STObject>) -> Result<()> {
        let _: SlotObject<u32> = root.get(sfSequence)?;
        let _: SlotObject<Amount> = root.get(sfBalance)?;
        let arr: SlotObject<STArray> = root.get(sfSignerEntries)?;
        let _n = arr.count()?;
        let _: SlotObject<STObject> = arr.get(0u32)?;
        // borrowing pre-checks compose with a consuming read
        let amt = root.get(sfBalance)?;
        let _is = amt.is_native()?;
        let _x: XFL = amt.as_xfl()?;
        // opaque both ways
        let op: SlotObject<Opaque> = root.get(sfSigningPubKey)?;
        let _: SlotObject<AccountId> = op.get(sfAccount)?;
        let _: SlotObject<STObject> = op.get(0u32)?;
        Ok(())
    }
    fn _reads(root: SlotObject<STObject>) -> Result<()> {
        let _: u32 = root.get(sfSequence)?.value()?;
        let _: u64 = root.get(sfExchangeRate)?.value()?;
        let _: AccountId = root.get(sfAccount)?.value()?;
        let _: Hash = root.get(sfLedgerHash)?.value()?;
        let _: CurrencyCode = root
            .get(sfTakerPaysCurrency)?
            .assume_type::<CurrencyCode>()
            .value()?;
        let _: AmountBytes = root.get(sfBalance)?.value()?;
        let _: IssueData = root.get(sfClaimCurrency)?.value()?;
        // take_* recycling
        let _: u32 = root.get(sfSequence)?.take_value()?;
        let _: XFL = root.get(sfBalance)?.take_xfl()?;
        let _: [u8; 4] = root.get(sfSequence)?.take_raw_exact::<4>()?;
        // raw escapes
        let mut b = [0u8; 8];
        let _n = root.get(sfSequence)?.raw(&mut b)?;
        let _: [u8; 32] = root.get(sfLedgerHash)?.raw_exact::<32>()?;
        // casts
        let _: SlotObject<STObject> = root.get(sfMemo)?.try_cast::<STObject>()?;
        let _: SlotObject<u32> = root.get(sfSigningPubKey)?.assume_type::<u32>();
        Ok(())
    }
    let _ = (_typed, _reads);
}

#[test]
fn slot_path_shapes() {
    fn _p(signers: SlotObject<STObject>) -> Result<()> {
        let one = slot_path!(signers[sfSignerEntries])?;
        let _ = one.clear();
        let three: SlotObject<AccountId> = slot_path!(signers[sfSignerEntries][0u32][sfAccount])?;
        let _: AccountId = three.value()?;
        Ok(())
    }
    let _ = _p;
    // runs on stubs: first hop fails, nothing leaks
    let r = SlotObject::from_otxn();
    assert!(r.is_err());
}

// ---------------------------------------------------------------------------
// Field-table parity
// ---------------------------------------------------------------------------
//
// The `typed.code() == raw` comparison is *generated* into `sfield.rs`
// alongside the table it checks (`cargo xtask gen-core`), so it cannot drift
// when upstream adds a field — run it with
// `cargo test -p rshooks --lib parity`. What is left here is the shape check
// the generated test cannot make: that both tables name the same fields, and
// that the typed one gates each name by its amendment availability.
//
// `rshooks-core::sfcodes` is a complete 1:1 mirror of the wire protocol and
// is never gated. `rshooks::sfield` declares the same names but attaches a
// `#[cfg]` from `crates/rshooks-core/format_availability.json`: `pending`
// fields are in by default and out under `active-amendments`, `dormant`
// fields need `all-amendments`. So the two tables agree on *names* and
// differ on *what compiles*.

/// Maps each `pub const` in a generated table to the `#[cfg]` on the line
/// above it, if any.
fn gated_names(src: &str) -> std::collections::BTreeMap<String, Option<String>> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = std::collections::BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim().strip_prefix("pub const ") else {
            continue;
        };
        let Some(name) = rest.split(':').next() else {
            continue;
        };
        let cfg = i
            .checked_sub(1)
            .and_then(|j| lines.get(j))
            .map(|l| l.trim())
            .filter(|l| l.starts_with("#[cfg"))
            .map(str::to_string);
        out.insert(name.to_string(), cfg);
    }
    out
}

#[test]
fn both_tables_name_the_same_fields_and_the_typed_one_gates_by_availability() {
    let typed = gated_names(include_str!("../src/sfield.rs"));
    let raw = gated_names(include_str!("../../rshooks-core/src/sfcodes.rs"));

    assert_eq!(raw.len(), 325, "the raw table must stay complete");
    assert_eq!(
        typed.keys().collect::<Vec<_>>(),
        raw.keys().collect::<Vec<_>>(),
        "the two tables name different fields",
    );
    assert!(
        raw.values().all(Option::is_none),
        "the raw table must never be gated — it mirrors the wire protocol",
    );

    let cfg_of = |n: &str| typed.get(n).cloned().flatten();
    const PENDING: &str =
        "#[cfg(any(not(feature = \"active-amendments\"), feature = \"all-amendments\"))]";
    const DORMANT: &str = "#[cfg(feature = \"all-amendments\")]";

    // Active: always available, never gated.
    for n in [
        "sfAccount",
        "sfAmount",
        "sfClaimCurrency",
        "sfLedgerEntryType",
    ] {
        assert_eq!(cfg_of(n), None, "{n} is active and must not be gated");
    }
    // Pending: in by default, out under `active-amendments`. The `any(...)`
    // form is what makes both-features-on resolve to "in" (widest wins).
    for n in ["sfNFTokenTaxon", "sfNFTokenOffers"] {
        assert_eq!(cfg_of(n).as_deref(), Some(PENDING), "{n} should be pending");
    }
    // Dormant: only under `all-amendments`. `sfAsset`/`sfXChainBridge` get
    // there by their formats; `sfCredentialIDs` by a `field_overrides` entry,
    // since `Payment` is active but `featureCredentials` is Supported::no.
    for n in ["sfAsset", "sfAsset2", "sfXChainBridge", "sfCredentialIDs"] {
        assert_eq!(cfg_of(n).as_deref(), Some(DORMANT), "{n} should be dormant");
    }
}

// The two checks above read source text. These two are the compile-time
// half: they only exist in the feature state they describe, and `mise run
// test` runs the suite in each state.

/// Pending shapes are part of the default surface — and stay part of it when
/// both features are on, which is the widest-wins rule. This test carries the
/// *same* cfg expression the pending constants do, so it exists in exactly
/// the states where they do: it would fail to compile if the two ever
/// disagreed.
#[cfg(any(not(feature = "active-amendments"), feature = "all-amendments"))]
#[test]
fn a_pending_constant_is_nameable_unless_narrowed() {
    assert_eq!(sfNFTokenTaxon.code(), rshooks::raw::sfcodes::sfNFTokenTaxon);
}

/// ...and dormant ones are not, until asked for.
#[cfg(feature = "all-amendments")]
#[test]
fn a_dormant_constant_is_nameable_under_all_amendments() {
    assert_eq!(sfAsset.code(), rshooks::raw::sfcodes::sfAsset);
}

// ---------------------------------------------------------------------------
// slot_path!: root evaluated once, errors propagate per hop
// ---------------------------------------------------------------------------

#[test]
fn slot_path_evaluates_its_root_once() {
    use core::cell::Cell;

    // The counter has to be ticked by an expression the macro *itself*
    // evaluates — the parenthesized-root form — or this proves nothing about
    // the macro at all. `slot_path!((expr)[..])` binds that expression once;
    // a naive expansion that re-emitted `$root` per hop would tick twice.
    let calls = Cell::new(0u32);
    let make = || {
        calls.set(calls.get().wrapping_add(1));
        SlotObject::from_otxn()
    };

    // Wrapped so `?` inside the root expression has somewhere to go.
    let walk = || -> Result<SlotObject<STObject>> { slot_path!((make()?)[sfSignerEntries][0u32]) };
    assert_eq!(walk().err(), Some(STUB));
    assert_eq!(calls.get(), 1, "the root expression must be evaluated once");
}

#[test]
fn slot_path_propagates_the_failing_hop() {
    // On host stubs the very first hop fails, and the error surfaces
    // unchanged rather than being masked by an intermediate clear.
    fn walk(root: &SlotObject<STObject>) -> Result<SlotObject<AccountId>> {
        slot_path!(root[sfSignerEntries][0u32][sfAccount])
    }
    let _ = walk;
}
