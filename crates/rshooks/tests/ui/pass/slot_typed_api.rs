//! The typed slot API's working shapes, compiled and run on host stubs: the
//! constructors, parent-gated navigation, the whole read table, the `take_*`
//! recycling family, both retypes, and `slot_path!`.
//!
//! Every call fails at the host stub — that is fine and is not what this
//! pins. What it pins is that the surface *exists* with these signatures and
//! that every value type is inferred from its field constant rather than
//! annotated.

use rshooks::prelude::*;
use rshooks::types::{Amount, Issue, Opaque};
use rshooks::slot_path;

fn reads(root: SlotObject<STObject>) -> Result<()> {
    // Value types come from the field constants.
    let _: u8 = root.get(sfCloseResolution)?.value()?;
    let _: u16 = root.get(sfTransactionType)?.value()?;
    let _: u32 = root.get(sfSequence)?.value()?;
    // `u64` reads its wire bytes, not as-int64: bit 63 is reachable here
    // (`sfExchangeRate` sets it) and the host rejects that in as-int64 mode.
    // The same eight bytes read raw must decode identically — that identity
    // is what the wire-bytes path buys, and it is why `value()` here is not
    // `slot_u64`.
    let _: u64 = root.get(sfExchangeRate)?.value()?;
    let _: u64 = root
        .get(sfExchangeRate)?
        .raw_exact::<8>()
        .map(u64::from_be_bytes)?;
    let _: u64 = root.get(sfExchangeRate)?.take_value()?;
    let _: Hash = root.get(sfLedgerHash)?.value()?;
    let _: AccountId = root.get(sfAccount)?.value()?;
    let _: AmountBytes = root.get(sfBalance)?.value()?;
    let _: IssueData = root.get(sfClaimCurrency)?.value()?;

    // Borrowing pre-checks compose with a consuming read.
    let amount = root.get(sfBalance)?;
    let _size = amount.size()?;
    let _native: bool = amount.is_native()?;
    let _: XFL = amount.as_xfl()?;

    // `is_native` is on `Opaque` too — an untyped slot is exactly the case
    // where the caller does not yet know what is in it, so asking is the
    // point (design §3.2 lists Amount *and* Opaque).
    let untyped: SlotObject<Opaque> = root.get(sfSigningPubKey)?;
    let _native: bool = untyped.is_native()?;
    let _count = untyped.count()?;
    let _: XFL = untyped.assume_type::<Amount>().as_xfl()?;

    // `field_code` hands back an erased `SField`, comparable against a
    // generated constant either way round, and still unwrappable to a code.
    let balance = root.get(sfBalance)?;
    let code: SField<Opaque> = balance.field_code()?;
    let _ = code == sfBalance;
    let _ = sfBalance == code;
    let _: u32 = code.code();

    // Raw escapes.
    let mut buf = [0u8; 32];
    let _n = root.get(sfLedgerHash)?.raw(&mut buf)?;
    let _: [u8; 32] = root.get(sfLedgerHash)?.raw_exact::<32>()?;

    // Recycling forms: read and release in one go.
    let _: u32 = root.get(sfSequence)?.take_value()?;
    let _: XFL = root.get(sfBalance)?.take_xfl()?;
    let _: [u8; 4] = root.get(sfSequence)?.take_raw_exact::<4>()?;

    Ok(())
}

fn navigation(root: SlotObject<STObject>) -> Result<()> {
    let entries: SlotObject<STArray> = root.get(sfSignerEntries)?;
    let _count = entries.count()?;
    let entry: SlotObject<STObject> = entries.get(0u32)?;
    let _: AccountId = entry.get(sfAccount)?.value()?;

    // Opaque navigates either way — the host decides at runtime.
    let opaque: SlotObject<Opaque> = root.get(sfSigningPubKey)?;
    let _: SlotObject<AccountId> = opaque.get(sfAccount)?;
    Ok(())
}

fn retypes(root: SlotObject<STObject>) -> Result<()> {
    // Checked: any failure consumes the handle and clears the slot.
    let _: SlotObject<STObject> = root.get(sfMemo)?.try_cast::<STObject>()?;
    let _: SlotObject<Amount> = root.get(sfBalance)?.try_cast::<Amount>()?;
    let _: SlotObject<Issue> = root.get(sfClaimCurrency)?.try_cast::<Issue>()?;
    // Unchecked, const: the escape for a field this layer types `Opaque`.
    let _: SlotObject<CurrencyCode> = root.get(sfTakerPaysCurrency)?.assume_type();
    Ok(())
}

fn paths(root: SlotObject<STObject>) -> Result<()> {
    // One hop, three hops, and a parenthesized-expression root.
    let _ = slot_path!(root[sfSignerEntries])?.clear();
    let _: AccountId = slot_path!(root[sfSignerEntries][0u32][sfAccount])?.value()?;
    let _ = slot_path!((SlotObject::from_otxn()?)[sfMemos][0u32])?.clear();
    Ok(())
}

fn main() {
    // Constructors: all four hand back an object handle.
    let _: Result<SlotObject<STObject>> = SlotObject::from_otxn();
    let _: Result<SlotObject<STObject>> = SlotObject::from_meta();
    let _: Result<SlotObject<STObject>> = SlotObject::from_keylet(&Keylet::default());
    let _: Result<SlotObject<STObject>> = SlotObject::from_txn_hash(&Hash::default());

    assert_eq!(
        SlotObject::from_otxn().err(),
        Some(HookError::NotImplemented)
    );

    let _ = (reads, navigation, retypes, paths);
}
