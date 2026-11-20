#!/usr/bin/env bash
# Build or clean the voice stack: the iroh C FFI dylib + the `eve-idfon-voice`
# holder binary, the eve-idfon-voice integration, and the `voice` agent bundle.
#
#   pnpm voice build
#   pnpm voice clean
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$root/scripts/lib/core.sh"
cd "$root"
action=${1:-}

case "$action" in
  build)
    # dylib first (install_name keeps it resolvable next to the executable)
          core_cargo build --release -p iroh-c-ffi
    cargo build --release -p idfon-voice-agent --features gpt-live
    # ad-hoc sign after relink, or the kernel SIGKILLs at exec. The dylib
    # lives in the vendored crate's own target/ (workspace-excluded); idfond's
    # build.rs copies it into "$core_target"/release/ later, so sign the source copy.
    codesign --force -s - \
      "$core_target"/release/libiroh_c_ffi.dylib \
      "$core_target"/release/eve-idfon-voice
    bash scripts/agent.sh build voice
    ;;
  clean)
    pnpm --filter eve-idfon-voice clean
    bash scripts/agent.sh clean voice
    cargo clean -p idfon-voice-agent
    cargo clean -p idfon-voice
    # cargo clean -p keeps the linked bin in "$core_target"/release
    rm -f "$core_target"/release/eve-idfon-voice
    ;;
  *)
    printf 'Usage: pnpm voice {build|clean}\n' >&2
    exit 2
    ;;
esac
