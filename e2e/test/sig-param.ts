// Shared helpers for the Hook Parameter Signature Interface
// (docs/PARAM_SIGNATURE_DESIGN.md): builds a declared `HookParameterName`
// (`0x5F 0x5F | index | 0x5F | type | 0x5F | name`, 7..=22 bytes) and
// big-endian-encodes invocation values — the TS-side mirror of
// `rshooks::sig::sig_param_name`/`sig_name!` on the Rust side (see
// `crates/rshooks/src/sig.rs`). Shared by every e2e suite exercising a
// `#[hook(..)]` entry with signature-parameter fn arguments: param-signature.

// The `STI_*` type bytes this interface supports
// (docs/PARAM_SIGNATURE_DESIGN.md §2's table) that these suites need.
export const STI_UINT8 = 0x10
export const STI_UINT16 = 0x01
export const STI_UINT64 = 0x03

/** Builds one declared `HookParameterName`, as uppercase hex. */
export function sigParamName(index: number, typeByte: number, name: string): string {
  const bytes = Buffer.concat([
    Buffer.from([0x5f, 0x5f, index, 0x5f, typeByte, 0x5f]),
    Buffer.from(name, 'ascii'),
  ])
  return bytes.toString('hex').toUpperCase()
}

/** Big-endian `u8` invocation value, as uppercase hex (1 byte). */
export function u8Hex(value: number): string {
  return Buffer.from([value]).toString('hex').toUpperCase()
}

/** Big-endian `u16` invocation value, as uppercase hex (2 bytes). */
export function u16BEHex(value: number): string {
  const buf = Buffer.alloc(2)
  buf.writeUInt16BE(value)
  return buf.toString('hex').toUpperCase()
}

/** Big-endian `u64` invocation value, as uppercase hex (8 bytes). */
export function u64BEHex(value: bigint): string {
  const buf = Buffer.alloc(8)
  buf.writeBigUInt64BE(value)
  return buf.toString('hex').toUpperCase()
}

/** One `HookParameter` array entry: `{ HookParameterName, HookParameterValue }`. */
export function sigParam(nameHex: string, valueHex: string) {
  return {
    HookParameter: {
      HookParameterName: nameHex,
      HookParameterValue: valueHex,
    },
  }
}
