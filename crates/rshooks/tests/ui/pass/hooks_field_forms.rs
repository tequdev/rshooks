//! Every `#[hooks]` chain-struct field-attribute form: `key`/`key_by`
//! (constant vs. keyed state), `name`/`name_by` (constant vs. keyed hook
//! param), and the param-only `required`/`default` modifiers, on `State`,
//! `HookParam`, and `OtxnParam` fields respectively.

use rshooks::prelude::*;
use rshooks::hooks;

/// A named constant used by the `key = <const expr>` form below.
const COUNTER_TAG: [u8; 2] = *b"CT";

#[hooks]
struct Vault {
    /// `key = <byte-string literal>` — a constant key.
    #[state(key = b"RR")]
    reward_rate: State<u64>,

    /// `key = <const expr>` — a constant key computed from a named const,
    /// not just a literal.
    #[state(key = &COUNTER_TAG)]
    counter: State<u64>,

    /// `key_by = <TypePath>` — a keyed state family.
    #[state(key_by = [u8; 20])]
    balances: State<u64>,

    /// `name = <byte-string literal>` — a constant, zero-copy param name.
    #[hook_param(name = b"CFG")]
    cfg: HookParam<[u8; 4]>,

    /// `name_by = <TypePath>` — a keyed param-name family.
    #[hook_param(name_by = [u8; 1])]
    seat: HookParam<AccountId>,

    /// `required` — absence is `HookError::DoesntExist`.
    #[hook_param(name = b"REQ", required)]
    required_param: HookParam<[u8; 4]>,

    /// `default = <expr>` — absence substitutes the given value.
    #[otxn_param(name = b"DEF", default = [0u8; 4])]
    default_param: OtxnParam<[u8; 4]>,
}

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

fn main() {
    assert!(Vault.state.reward_rate.get().is_err());
    assert!(Vault.state.counter.get().is_err());
    assert!(Vault.state.balances.at([0u8; 20]).get().is_err());
    assert!(Vault.hook_param.cfg.get().is_err());
    assert!(Vault.hook_param.seat.at([0u8]).get().is_err());
    assert!(Vault.hook_param.required_param.get_required().is_err());
    // `NotImplemented` (the host stub every Hook API call returns on a host
    // build) is a decode/host error, not absence — so `default` is never
    // substituted here; see `decl`'s module doc comment, "Params: absence
    // vs. decode failure".
    assert!(Vault.otxn_param.default_param.get_or_default().is_err());
}
