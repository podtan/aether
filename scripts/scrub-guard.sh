#!/usr/bin/env bash
#
# scrub-guard.sh — history scrub guard for this public repository.
#
# WHY: the scrub law. This is a public opensource repository: history must
# stay byte-clean — file contents AND commit messages. Never commit internal
# hostnames, references to the platform's operating company, or secrets.
# Internal deployment config lives in the deploy infra, never here; examples
# use example.com placeholders only. A scrub regression means history
# rewrite, which is a one-way door.
#
# MODES
#   msg <file>            Scan one commit-message file (commit-msg hook entry).
#   range <base> <head>   Scan every commit in <base>..<head>: commit messages
#                         plus every blob introduced by those commits (CI entry).
#   full                  Audit mode: every commit message on every ref plus
#                         every blob on every ref. Run before releases and
#                         after any history operation. Exits 1 on ANY finding.
#
# NOTE ON THE DENY LIST: the patterns below are assembled from fragments at
# runtime so that THIS SCRIPT's own blobs stay clean under its own audit —
# the guard must never become a finding of itself, and the audit counts must
# not be polluted. Do NOT inline the literals anywhere in the repository.
# Secret-shape detection is best-effort; human review at release remains the
# durable gate.
#
# Exit codes: 0 = clean, 1 = findings, 2 = usage error.

set -u

# --- deny list (fragmented; never inline the literals) ---
d1="tan""bal"  # internal platform domain label, first host family
d2="p""od""ai" # internal platform domain label, second host family
d3="p""od"     # internal platform / operating-company label

HOST_RE="([a-z0-9_-]+\\.)*(${d1}|${d2}|${d3})\\.ir"
ORG_RE="\\b${d3}\\b"

SECRET_RES=(
  '-----BEGIN [A-Z ]*PRIVATE KEY-----'
  'AKIA[0-9A-Z]{16}'
  '\bgh[pousr]_[A-Za-z0-9]{30,}'
  '\bxox[baprs]-[A-Za-z0-9-]{10,}'
  '\bsk-[A-Za-z0-9]{20,}'
  '\bglpat-[A-Za-z0-9_-]{20,}'
)

# scan_text <label>  (reads text on stdin; prints findings; 0 clean, 1 findings)
scan_text() {
  local label="$1" rc=0 out sre input
  # Slurp stdin ONCE: a second grep on the raw pipe would scan empty input
  # (the first grep drains it) and silently skip its pattern class.
  input=$(cat)
  # -a: scan binary blobs too — a hostname inside a binary must still be a
  # finding (fail loud, never silently skip).
  if out=$(grep -anEi -- "$HOST_RE" <<<"$input"); then
    echo "SCRUB-BLOCK [$label] internal-hostname pattern:"
    echo "$out"
    rc=1
  fi
  if out=$(grep -anEi -- "$ORG_RE" <<<"$input"); then
    echo "SCRUB-BLOCK [$label] operating-company reference pattern:"
    echo "$out"
    rc=1
  fi
  for sre in "${SECRET_RES[@]}"; do
    if out=$(grep -anE -- "$sre" <<<"$input"); then
      echo "SCRUB-BLOCK [$label] secret-shape pattern:"
      echo "$out"
      rc=1
    fi
  done
  return "$rc"
}

# range_blobs <base> <head> — blob shas introduced by commits in the range
range_blobs() {
    for sha in $(git rev-list "$1..$2"); do
      git diff-tree -r -m --root --no-commit-id "$sha" \
        | awk '$4 ~ /^[0-9a-f]{40}$/ && $4 !~ /^0+$/ && $2 != "160000" {print $4}'
    done | sort -u
}

usage() {
  echo "usage: scrub-guard.sh msg <file>" >&2
  echo "       scrub-guard.sh range <base> <head>" >&2
  echo "       scrub-guard.sh full" >&2
}

mode="${1:-}"
case "$mode" in
  msg)
    file="${2:-}"
    [ -n "$file" ] && [ -f "$file" ] || { usage; exit 2; }
    scan_text "commit-message" < "$file"
    exit $?
    ;;
  range)
    # Two forms: range <base> <head>  (CI)  or  range <base>..<head>  (git notation)
    if [ -n "${3:-}" ]; then
      base="$2"; headrev="$3"
    elif [ -n "${2:-}" ] && [ "${2#*..}" != "$2" ]; then
      base="${2%%..*}"; headrev="${2#*..}"
    else
      usage; exit 2
    fi
    rc=0
    for sha in $(git rev-list "$base..$headrev"); do
      git log -1 --format=%B "$sha" | scan_text "message $sha" || rc=1
    done
    for b in $(range_blobs "$base" "$headrev"); do
      git cat-file blob "$b" | scan_text "blob $b" || rc=1
    done
    exit $rc
    ;;
  full)
    rc=0
    for sha in $(git rev-list --all); do
      git log -1 --format=%B "$sha" | scan_text "message $sha" || rc=1
    done
    for b in $(git rev-list --objects --all \
               | awk '{print $1}' | grep -E '^[0-9a-f]{40}$' | sort -u); do
      git cat-file blob "$b" 2>/dev/null | scan_text "blob $b" || rc=1
    done
    exit $rc
    ;;
  *)
    usage
    exit 2
    ;;
esac
