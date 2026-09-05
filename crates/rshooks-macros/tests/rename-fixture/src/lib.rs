//! Fixture for `rename_fixture.rs`: depends on `rshooks` renamed to
//! `hooks` (`hooks = { package = "rshooks", .. }`), exercising every
//! generator whose output has to reference the `rshooks` crate under
//! whatever name this `Cargo.toml` actually gives it — regression coverage
//! for `krate::rewrite`/`krate::extend_path_tokens`.

use hooks::exit::{Accept, HookResult};
use hooks::hooks;
use hooks::{HookData, HookKey, ParamName, ParamValue, XFL, account_id};

#[derive(HookData)]
struct Balance {
    amount: u64,
}

#[derive(HookKey)]
struct BalanceKey {
    id: u32,
}

#[derive(ParamName)]
struct Tag {
    kind: u8,
}

#[derive(ParamValue)]
struct Payload {
    value: u32,
}

const OWNER: hooks::types::AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");
const RATE: hooks::xfl::XFL = XFL!(0.5);

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

#[allow(dead_code)]
fn use_all() -> (Balance, BalanceKey, Tag, Payload) {
    let _ = OWNER;
    let _ = RATE;
    (
        Balance { amount: 0 },
        BalanceKey { id: 0 },
        Tag { kind: 0 },
        Payload { value: 0 },
    )
}
