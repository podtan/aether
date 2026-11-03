#!/usr/bin/env bash
#
# install-hooks.sh — wire this clone's git hooks to the repo's hooks/ dir.
# Local config only (core.hooksPath=hooks); safe to re-run.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
if [ ! -x "$root/hooks/commit-msg" ]; then
  echo "error: hooks/commit-msg missing or not executable" >&2
  exit 1
fi
git -C "$root" config core.hooksPath hooks
echo "scrub guard hooks installed (core.hooksPath=hooks)"
