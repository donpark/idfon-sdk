# Sourced by idfon-sdk scripts after they set $root (this repo's root).
#
# idfon-sdk builds against a sibling idfon-core checkout (default
# ../idfon-core; set IDFON_CORE to use another). Both workspaces share core's
# target dir (.cargo/config.toml), so `idfon`, `idfond` and libiroh_c_ffi end
# up side by side in "$core_target/release" — the CLI auto-spawns the daemon from
# its own directory.
#
# Local keys: "$root/.env" (gitignored) holds KEY=VALUE lines (a leading
# `export ` and surrounding quotes are fine), e.g. AI_GATEWAY_API_KEY. The
# scripts read it; agents only inherit the environment. A variable already set
# wins. POSIX sh: most callers are #!/bin/sh.
if [ -r "$root/.env" ]; then
  while IFS= read -r dotenv_line || [ -n "$dotenv_line" ]; do
    dotenv_line=${dotenv_line#export }
    dotenv_key=${dotenv_line%%=*}
    [ "$dotenv_key" != "$dotenv_line" ] || continue
    case $dotenv_key in ''|[0-9]*|*[!A-Za-z0-9_]*) continue ;; esac
    dotenv_value=${dotenv_line#*=}
    case $dotenv_value in
      \"*\") dotenv_value=${dotenv_value#\"}; dotenv_value=${dotenv_value%\"} ;;
      \'*\') dotenv_value=${dotenv_value#\'}; dotenv_value=${dotenv_value%\'} ;;
    esac
    eval "dotenv_set=\${$dotenv_key+x}"
    [ -n "$dotenv_set" ] || export "$dotenv_key=$dotenv_value"
  done < "$root/.env"
  unset dotenv_line dotenv_key dotenv_value dotenv_set
fi
core="${IDFON_CORE:-$root/../idfon-core}"
if ! core="$(CDPATH= cd -- "$core" 2>/dev/null && pwd)"; then
  echo "idfon-core not found at ${IDFON_CORE:-$root/../idfon-core} (clone it beside idfon-sdk or set IDFON_CORE)" >&2
  exit 1
fi
core_target="$core/target"
# Runs cargo inside idfon-core (core crates: idfond, iroh-c-ffi, idfon-mcp, …).
core_cargo() { (cd "$core" && cargo "$@"); }
