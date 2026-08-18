//! Builds the `ClaimReward` wait-time rollback message.

/// `"You must wait 0000000 seconds"` — digits at indices 14..=20.
const TEMPLATE: [u8; 29] = *b"You must wait 0000000 seconds";

/// Replaces a template digit.
fn bump_digit(msg: &mut [u8; 29], index: usize, digit: u8) {
    if let Some(byte) = msg.get_mut(index) {
        *byte = byte.wrapping_add(digit);
    }
}

/// Builds the wait-time rollback message.
#[must_use]
pub fn wait_message(remaining_seconds: u64) -> [u8; 29] {
    let mut msg = TEMPLATE;
    let r = remaining_seconds;
    bump_digit(
        &mut msg,
        14,
        r.wrapping_div(1_000_000).wrapping_rem(10) as u8,
    );
    bump_digit(&mut msg, 15, r.wrapping_div(100_000).wrapping_rem(10) as u8);
    bump_digit(&mut msg, 16, r.wrapping_div(10_000).wrapping_rem(10) as u8);
    bump_digit(&mut msg, 17, r.wrapping_div(1_000).wrapping_rem(10) as u8);
    bump_digit(&mut msg, 18, r.wrapping_div(100).wrapping_rem(10) as u8);
    bump_digit(&mut msg, 19, r.wrapping_div(10).wrapping_rem(10) as u8);
    bump_digit(&mut msg, 20, r.wrapping_rem(10) as u8);
    msg
}
