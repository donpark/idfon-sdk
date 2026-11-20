# Sourced by idfon-sdk scripts after they set $root (this repo's root).
#
# idfon-sdk builds against a sibling idfon-core checkout (default
# ../idfon-core; set IDFON_CORE to use another). Both workspaces share core's
# target dir (.cargo/config.toml), so `idfon`, `idfond` and libiroh_c_ffi end
# up side by side in "$core_target/release" — the CLI auto-spawns the daemon from
# its own directory.
core="${IDFON_CORE:-$root/../idfon-core}"
if ! core="$(CDPATH= cd -- "$core" 2>/dev/null && pwd)"; then
  echo "idfon-core not found at ${IDFON_CORE:-$root/../idfon-core} (clone it beside idfon-sdk or set IDFON_CORE)" >&2
  exit 1
fi
core_target="$core/target"
# Runs cargo inside idfon-core (core crates: idfond, iroh-c-ffi, idfon-mcp, …).
core_cargo() { (cd "$core" && cargo "$@"); }
