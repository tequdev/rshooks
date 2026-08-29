#!/usr/bin/env sh
# Sync or verify vendored xahaud sources and their SHA256 checksums.
#
# Usage:
#   scripts/sync-vendor.sh           # update: download from upstream,
#                                    # overwrite vendored files, regenerate
#                                    # each group's SHA256SUMS (review with
#                                    # `git diff`)
#   scripts/sync-vendor.sh --check   # verify: fail (exit 1) if any group's
#                                    # vendored files differ from the
#                                    # upstream release branch or from its
#                                    # SHA256SUMS; writes nothing. Used by CI.
set -eu

REPO="Xahau/xahaud"
BRANCH="release"

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

MODE="update"
if [ "${1:-}" = "--check" ]; then
    MODE="check"
elif [ -n "${1:-}" ]; then
    echo "usage: $0 [--check]" >&2
    exit 2
fi

# Print a SHA256 digest on macOS and Linux.
sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT INT TERM

overall_status=0

# Sync one vendor directory from one upstream tree.
#
#   sync_group <label> <vendor-dir> <upstream-dir> <file>...
#
# Each <file> is resolved against <upstream-dir> upstream but vendored (and
# recorded in SHA256SUMS) under its basename, so a group whose files live in
# more than one upstream directory passes an empty <upstream-dir> and
# repo-root-relative paths instead. For a group whose files all share one
# directory — the two header groups below — basename is the identity and
# this behaves exactly as before.
sync_group() {
    name="$1"
    vendor_rel="$2"
    vendor_dir="${ROOT_DIR}/${vendor_rel}"
    upstream_path="$3"
    shift 3
    files="$*"

    base_url="https://raw.githubusercontent.com/${REPO}/refs/heads/${BRANCH}"
    if [ -n "${upstream_path}" ]; then
        base_url="${base_url}/${upstream_path}"
    fi
    sums_file="${vendor_dir}/SHA256SUMS"
    group_tmp="${TMP_DIR}/${name}"
    mkdir -p "${group_tmp}"

    echo "[${name}] fetching from ${REPO}@${BRANCH}/${upstream_path} ..."
    for f in ${files}; do
        b="$(basename "${f}")"
        if ! curl -sfL "${base_url}/${f}" -o "${group_tmp}/${b}"; then
            echo "error: failed to download ${base_url}/${f}" >&2
            exit 1
        fi
    done

    if [ "${MODE}" = "check" ]; then
        group_status=0

        for f in ${files}; do
            b="$(basename "${f}")"
            if ! cmp -s "${group_tmp}/${b}" "${vendor_dir}/${b}"; then
                echo "DRIFT: [${name}] ${b} differs from upstream ${REPO}@${BRANCH}" >&2
                diff -u "${vendor_dir}/${b}" "${group_tmp}/${b}" | head -40 >&2 || true
                group_status=1
            fi
        done

        for f in ${files}; do
            b="$(basename "${f}")"
            want="$(awk -v f="${b}" '$2 == f {print $1}' "${sums_file}")"
            got="$(sha256 "${vendor_dir}/${b}")"
            if [ "${want}" != "${got}" ]; then
                echo "DRIFT: [${name}] ${b} does not match SHA256SUMS (want ${want}, got ${got})" >&2
                group_status=1
            fi
        done

        if [ "${group_status}" -eq 0 ]; then
            echo "OK: [${name}] vendored files are byte-identical to ${REPO}@${BRANCH} and match SHA256SUMS"
        else
            overall_status=1
        fi
        return 0
    fi

    changed=0
    for f in ${files}; do
        b="$(basename "${f}")"
        if ! cmp -s "${group_tmp}/${b}" "${vendor_dir}/${b}"; then
            cp "${group_tmp}/${b}" "${vendor_dir}/${b}"
            echo "[${name}] updated: ${b}"
            changed=1
        else
            echo "[${name}] unchanged: ${b}"
        fi
    done

    : > "${sums_file}"
    for f in ${files}; do
        b="$(basename "${f}")"
        printf '%s  %s\n' "$(sha256 "${vendor_dir}/${b}")" "${b}" >> "${sums_file}"
    done

    if [ "${changed}" -eq 1 ]; then
        echo "[${name}] vendored files updated. Review with:  git diff ${vendor_rel}"
    else
        echo "[${name}] already in sync with ${REPO}@${BRANCH}"
    fi
}

sync_group "guard-checker" \
    "crates/rshooks-build/vendor/xahaud" \
    "include/xrpl/hook" \
    Guard.h Enum.h hook_api.macro

sync_group "hook-headers" \
    "crates/rshooks-core/vendor/xahaud-hook" \
    "hook" \
    error.h extern.h hookapi.h ls_flags.h macro.h sfcodes.h tts.h tx_flags.h

# Protocol format definitions. These span two upstream directories, so the
# group passes an empty upstream dir and full repo-relative paths; they land
# flat in the vendor directory under their basenames.
sync_group "protocol-formats" \
    "crates/rshooks-core/vendor/xahaud-protocol" \
    "" \
    include/xrpl/protocol/detail/sfields.macro \
    include/xrpl/protocol/detail/transactions.macro \
    include/xrpl/protocol/detail/ledger_entries.macro \
    src/libxrpl/protocol/TxFormats.cpp \
    src/libxrpl/protocol/LedgerFormats.cpp \
    src/libxrpl/protocol/InnerObjectFormats.cpp

if [ "${MODE}" = "check" ]; then
    if [ "${overall_status}" -ne 0 ]; then
        echo "" >&2
        echo "Vendored sources have drifted. Run scripts/sync-vendor.sh to" >&2
        echo "re-sync from upstream, review the diff, and commit the result." >&2
    fi
    exit "${overall_status}"
fi

echo ""
echo "Done. Review any changes with:  git diff crates/rshooks-build/vendor/ crates/rshooks-core/vendor/"
echo "If the hook-headers or protocol-formats group changed, regenerate"
echo "rshooks-core's translated sources and format artifacts:"
echo "  cargo xtask gen-core"
echo "Then run the test suite (vendored behavior/translations may have changed):"
echo "  cargo test --workspace"
