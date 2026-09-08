// The only C++ this project authors (`docs/DESIGN.md` §6.5). Everything
// else in `guard_verdict` comes from the vendored, byte-identical upstream
// `xahaud` headers in `vendor/xahaud/` — this file just adapts their
// standalone-build entry point (`validateGuards`, built with
// `-DGUARD_CHECKER_BUILD`) to a small `extern "C"` ABI that `guard_native.rs`
// can call safely from Rust.
//
// Never hand-edit the vendored headers to make this compile; if the ABI
// upstream exposes ever changes, this file is what should change.

#define GUARD_CHECKER_BUILD
#include "Guard.h"

#include <cstdint>
#include <cstring>
#include <exception>
#include <sstream>
#include <string>

extern "C" {

// Runs the vendored upstream guard checker against `wasm[0..wasm_len)`.
//
// Returns:
//   0 — valid: `*out_hook_cost` / `*out_cbak_cost` are the worst-case
//       instruction counts `validateGuards` computed for `hook()` / `cbak()`,
//       and `*out_hook_exec_cost` / `*out_cbak_exec_cost` are its HookFeeV2
//       worst-case execution costs (instructions plus
//       `hook_api::api_call_cost` per Hook API call, in cost units).
//   1 — invalid: the module failed a guard-checker rule. The checker's log
//       (identical to what a real node would produce) is copied into
//       `log_buf`.
//   2 — the checker threw an exception (upstream documents `validateGuards`
//       "may throw overflow_error" on malformed LEB128 input, etc). The
//       exception's `what()` is copied into `log_buf`.
//
// `log_buf`/`log_cap`: destination buffer for the captured log text (UTF-8,
// NOT nul-terminated by this function — the caller has the byte count via
// `out_log_len` and should not assume a terminator). At most `log_cap` bytes
// are written into `log_buf`; `*out_log_len` is always set to the FULL
// length of the log text (even if that exceeds `log_cap`), so the caller can
// detect truncation by comparing `*out_log_len` against `log_cap`.
//
// No global state is touched; every call is independent and this function
// is safe to call concurrently from multiple threads.
int rshooks_validate_guards(
    const uint8_t* wasm,
    size_t wasm_len,
    uint64_t* out_hook_cost,
    uint64_t* out_cbak_cost,
    uint64_t* out_hook_exec_cost,
    uint64_t* out_cbak_exec_cost,
    char* log_buf,
    size_t log_cap,
    size_t* out_log_len)
{
    std::ostringstream log_stream;

    auto copy_log = [&](const std::string& text) {
        *out_log_len = text.size();
        size_t to_copy = text.size() < log_cap ? text.size() : log_cap;
        if (to_copy > 0)
        {
            std::memcpy(log_buf, text.data(), to_copy);
        }
    };

    try
    {
        std::vector<uint8_t> wasm_vec(wasm, wasm + wasm_len);

        std::ostream& log_ref = log_stream;
        GuardLog guard_log = std::make_optional(std::ref(log_ref));

        hook_api::Rules rules{};
        hook_api::APIWhitelist whitelist = hook_api::getImportWhitelist(rules);
        uint64_t rules_version = hook_api::getGuardRulesVersion(rules);

        auto verdict = validateGuards(
            wasm_vec,
            guard_log,
            std::string("rshooks-build"),
            /* returnCost */ false,
            whitelist,
            rules_version);

        copy_log(log_stream.str());

        if (!verdict)
        {
            return 1;
        }

        // `validateGuards` reports either the instruction count or the
        // execution cost per call, selected by `returnCost`; the second
        // pass re-runs the same checker (log suppressed) for the cost.
        auto cost = validateGuards(
            wasm_vec,
            std::nullopt,
            std::string("rshooks-build"),
            /* returnCost */ true,
            whitelist,
            rules_version);

        if (!cost)
        {
            return 1;
        }

        *out_hook_cost = verdict->first;
        *out_cbak_cost = verdict->second;
        *out_hook_exec_cost = cost->first;
        *out_cbak_exec_cost = cost->second;
        return 0;
    }
    catch (const std::exception& e)
    {
        std::string text = log_stream.str();
        text += "\nguard_shim: validateGuards threw: ";
        text += e.what();
        copy_log(text);
        return 2;
    }
    catch (...)
    {
        std::string text = log_stream.str();
        text += "\nguard_shim: validateGuards threw a non-std::exception value";
        copy_log(text);
        return 2;
    }
}

}  // extern "C"
