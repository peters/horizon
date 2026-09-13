#!/usr/bin/env bash
set -euo pipefail

# Orchestrate GitHub Release finalization for Horizon.
# The Git tag is source identity. The published GitHub Release is the
# completion record and must not become public until the required assets exist.

usage() {
  cat <<'EOF'
Usage:
  release-github.sh ensure-pending --repo <owner/name> --tag <tag> --commit <sha> --prerelease <true|false>
  release-github.sh upload-assets --repo <owner/name> --tag <tag> --commit <sha> --dir <dir>
  release-github.sh publish --repo <owner/name> --tag <tag> --commit <sha> --prerelease <true|false>
EOF
}

REPO=""
TAG=""
COMMIT=""
PRERELEASE=""
ASSET_DIR=""
WORKDIR=""

COMMIT_MARKER_PREFIX='<!-- horizon-release-commit: '
COMMIT_MARKER_SUFFIX=' -->'

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

require_commands() {
  local cmd
  for cmd in gh jq sha256sum; do
    command -v "$cmd" >/dev/null 2>&1 || die "Required command not found: $cmd"
  done
}

parse_bool() {
  case "$1" in
    true|false) printf '%s\n' "$1" ;;
    *) die "Expected true or false, got: $1" ;;
  esac
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --repo)
        REPO="${2:-}"
        shift 2
        ;;
      --tag)
        TAG="${2:-}"
        shift 2
        ;;
      --commit)
        COMMIT="${2:-}"
        shift 2
        ;;
      --prerelease)
        PRERELEASE="$(parse_bool "${2:-}")"
        shift 2
        ;;
      --dir)
        ASSET_DIR="${2:-}"
        shift 2
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        printf 'Unknown argument: %s\n\n' "$1" >&2
        usage >&2
        exit 1
        ;;
    esac
  done
}

require_identity() {
  [ -n "$REPO" ] || die "The --repo argument is required."
  [ -n "$TAG" ] || die "The --tag argument is required."
  [ -n "$COMMIT" ] || die "The --commit argument is required."
  [[ "$COMMIT" =~ ^[0-9a-fA-F]{40,64}$ ]] || die "The --commit argument must be a full Git object name."
}

normalize_sha() {
  tr 'A-F' 'a-f' <<<"$1"
}

file_digest() {
  local hex
  hex="$(sha256sum -- "$1" | awk '{ print $1 }')"
  printf 'sha256:%s\n' "$(normalize_sha "$hex")"
}

required_asset_names() {
  printf '%s\n' \
    horizon-linux-x64.tar.gz \
    horizon-osx-arm64.tar.gz \
    horizon-osx-x64.tar.gz \
    horizon-windows-x64.exe \
    SHA256SUMS.txt
  if [ "${PRERELEASE:-true}" = "false" ]; then
    printf '%s\n' \
      horizon-installer-linux-x64.bin \
      horizon-installer-osx-arm64.bin \
      horizon-installer-osx-x64.bin \
      horizon-installer-win-x64.exe
  fi
}

json_asset_digest() {
  local json="$1"
  local name="$2"
  jq -r --arg name "$name" '.assets[]? | select(.name == $name) | .digest // empty' <<<"$json"
}

json_has_asset() {
  local json="$1"
  local name="$2"
  jq -e --arg name "$name" '[.assets[]? | .name] | index($name) != null' <<<"$json" >/dev/null
}

missing_required_assets() {
  local json="$1"
  local name
  while IFS= read -r name; do
    if ! json_has_asset "$json" "$name"; then
      printf '%s\n' "$name"
    fi
  done < <(required_asset_names)
}

release_is_complete() {
  local json="$1"
  local missing
  missing="$(missing_required_assets "$json")"
  [ -z "$missing" ]
}

extract_recorded_commit() {
  local body="$1"
  sed -n "s/.*${COMMIT_MARKER_PREFIX}\\([0-9a-fA-F]\\{40,64\\}\\)${COMMIT_MARKER_SUFFIX}.*/\\1/p" <<<"$body" | head -n 1
}

body_with_commit() {
  local body="$1"
  local commit
  commit="$(normalize_sha "$2")"
  local marker="${COMMIT_MARKER_PREFIX}${commit}${COMMIT_MARKER_SUFFIX}"
  if grep -Fq "$COMMIT_MARKER_PREFIX" <<<"$body"; then
    printf '%s\n' "$body"
    return 0
  fi
  if [ -n "$body" ]; then
    printf '%s\n\n%s\n' "$body" "$marker"
  else
    printf '%s\n' "$marker"
  fi
}

load_release_json() {
  local err_file status
  err_file="$(mktemp "${WORKDIR}/err.XXXXXX")"
  if RELEASE_JSON="$(gh release view "$TAG" --repo "$REPO" --json isDraft,isPrerelease,body,assets,tagName 2>"$err_file")"; then
    rm -f "$err_file"
    return 0
  else
    status=$?
  fi
  if grep -qiE 'release not found|HTTP[[:space:]]*404|Not Found' "$err_file"; then
    rm -f "$err_file"
    RELEASE_JSON=""
    return 1
  fi
  cat "$err_file" >&2
  rm -f "$err_file"
  if [ "$status" -eq 1 ]; then
    return 2
  fi
  return "$status"
}

assert_recorded_commit() {
  local json="$1"
  local body recorded
  body="$(jq -r '.body // empty' <<<"$json")"
  recorded="$(extract_recorded_commit "$body")"
  if [ -z "$recorded" ]; then
    die "GitHub Release ${TAG} is missing the recorded source commit."
  fi
  if [ "$(normalize_sha "$recorded")" != "$(normalize_sha "$COMMIT")" ]; then
    die "GitHub Release ${TAG} was recorded for commit ${recorded}, not ${COMMIT}."
  fi
}

create_pending_release() {
  local notes_file
  notes_file="$(mktemp "${WORKDIR}/notes.XXXXXX")"
  body_with_commit "Release ${TAG}" "$COMMIT" >"$notes_file"
  local args=(
    release create "$TAG"
    --repo "$REPO"
    --draft
    --verify-tag
    --title "$TAG"
    --target "$COMMIT"
    --notes-file "$notes_file"
  )
  if [ "$PRERELEASE" = "true" ]; then
    args+=(--prerelease)
  fi
  gh "${args[@]}"
  rm -f "$notes_file"
  printf 'Created draft GitHub Release %s for commit %s.\n' "$TAG" "$(normalize_sha "$COMMIT")"
}

sync_pending_release() {
  local json="$1"
  local draft body recorded notes_file
  local need_draft=0
  local need_notes=0

  draft="$(jq -r '.isDraft' <<<"$json")"
  body="$(jq -r '.body // empty' <<<"$json")"
  recorded="$(extract_recorded_commit "$body")"

  if grep -Fq "$COMMIT_MARKER_PREFIX" <<<"$body" && [ -z "$recorded" ]; then
    die "GitHub Release ${TAG} has a malformed recorded source commit."
  fi
  if [ -n "$recorded" ] && [ "$(normalize_sha "$recorded")" != "$(normalize_sha "$COMMIT")" ]; then
    die "GitHub Release ${TAG} was recorded for commit ${recorded}, not ${COMMIT}."
  fi
  if [ -z "$recorded" ]; then
    need_notes=1
  fi

  if [ "$draft" != "true" ] && ! release_is_complete "$json"; then
    need_draft=1
  fi

  if [ "$need_draft" -eq 0 ] && [ "$need_notes" -eq 0 ]; then
    if [ "$draft" = "true" ]; then
      printf 'GitHub Release %s is already a draft for commit %s.\n' "$TAG" "$(normalize_sha "$COMMIT")"
    else
      printf 'GitHub Release %s is already published with the required asset set.\n' "$TAG"
    fi
    return 0
  fi

  if [ "$need_notes" -eq 1 ]; then
    notes_file="$(mktemp "${WORKDIR}/notes.XXXXXX")"
    body_with_commit "$body" "$COMMIT" >"$notes_file"
    gh release edit "$TAG" --repo "$REPO" --notes-file "$notes_file"
    rm -f "$notes_file"
  fi
  if [ "$need_draft" -eq 1 ]; then
    gh release edit "$TAG" --repo "$REPO" --draft
  fi

  if [ "$need_draft" -eq 1 ]; then
    printf 'Converted GitHub Release %s back to a draft until the required assets exist.\n' "$TAG"
  fi
  if [ "$need_notes" -eq 1 ]; then
    printf 'Recorded source commit %s on GitHub Release %s.\n' "$(normalize_sha "$COMMIT")" "$TAG"
  fi
}

load_existing_release() {
  local status
  set +e
  load_release_json
  status=$?
  set -e
  case "$status" in
    0) return 0 ;;
    1) die "GitHub Release ${TAG} does not exist." ;;
    *) exit "$status" ;;
  esac
}

cmd_ensure_pending() {
  local status
  parse_args "$@"
  require_identity
  [ -n "$PRERELEASE" ] || die "The --prerelease argument is required."

  set +e
  load_release_json
  status=$?
  set -e
  case "$status" in
    0) sync_pending_release "$RELEASE_JSON" ;;
    1) create_pending_release ;;
    *) exit "$status" ;;
  esac
}

sorted_asset_files() {
  local path base
  local checksum=""
  local files=()
  for path in "$ASSET_DIR"/*; do
    [ -f "$path" ] || continue
    base="$(basename "$path")"
    if [ "$base" = "SHA256SUMS.txt" ]; then
      checksum="$path"
      continue
    fi
    files+=("$path")
  done
  if [ "${#files[@]}" -gt 0 ]; then
    printf '%s\n' "${files[@]}" | LC_ALL=C sort
  fi
  if [ -n "$checksum" ]; then
    printf '%s\n' "$checksum"
  fi
}

replace_or_upload_asset() {
  local json="$1"
  local path="$2"
  local name digest remote_digest
  name="$(basename "$path")"
  digest="$(file_digest "$path")"

  if json_has_asset "$json" "$name"; then
    remote_digest="$(normalize_sha "$(json_asset_digest "$json" "$name")")"
    if [ -n "$remote_digest" ] && [ "$remote_digest" = "$(normalize_sha "$digest")" ]; then
      printf 'Skipping %s (digest matches).\n' "$name"
      return 0
    fi
    printf 'Replacing %s (digest changed for recorded commit).\n' "$name"
    gh release delete-asset "$TAG" "$name" --repo "$REPO" --yes
  else
    printf 'Uploading %s.\n' "$name"
  fi
  gh release upload "$TAG" "$path" --repo "$REPO"
}

cmd_upload_assets() {
  parse_args "$@"
  require_identity
  [ -n "$ASSET_DIR" ] || die "The --dir argument is required."
  [ -d "$ASSET_DIR" ] || die "Asset directory not found: $ASSET_DIR"

  load_existing_release
  assert_recorded_commit "$RELEASE_JSON"

  local path
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    replace_or_upload_asset "$RELEASE_JSON" "$path"
    load_existing_release
    assert_recorded_commit "$RELEASE_JSON"
  done < <(sorted_asset_files)

  printf 'Asset upload for %s is complete without publishing.\n' "$TAG"
}

cmd_publish() {
  parse_args "$@"
  require_identity
  [ -n "$PRERELEASE" ] || die "The --prerelease argument is required."

  load_existing_release
  assert_recorded_commit "$RELEASE_JSON"

  local missing draft
  missing="$(missing_required_assets "$RELEASE_JSON")"
  if [ -n "$missing" ]; then
    printf 'error: Refusing to publish GitHub Release %s with missing assets:\n' "$TAG" >&2
    printf '%s\n' "$missing" >&2
    exit 1
  fi

  draft="$(jq -r '.isDraft' <<<"$RELEASE_JSON")"
  if [ "$draft" != "true" ]; then
    printf 'GitHub Release %s is already published with the required asset set.\n' "$TAG"
    return 0
  fi

  local edit_args=(release edit "$TAG" --repo "$REPO" --draft=false)
  if [ "$PRERELEASE" = "true" ]; then
    edit_args+=(--prerelease)
  fi
  gh "${edit_args[@]}"
  printf 'Published GitHub Release %s after verifying the required asset set.\n' "$TAG"
}

main() {
  require_commands
  WORKDIR="$(mktemp -d)"
  trap 'rm -rf "$WORKDIR"' EXIT
  local command="${1:-}"
  if [ "$#" -gt 0 ]; then
    shift
  fi
  case "$command" in
    ensure-pending) cmd_ensure_pending "$@" ;;
    upload-assets) cmd_upload_assets "$@" ;;
    publish) cmd_publish "$@" ;;
    -h|--help|"")
      usage
      [ -n "$command" ] || exit 1
      ;;
    *)
      printf 'Unknown command: %s\n\n' "$command" >&2
      usage >&2
      exit 1
      ;;
  esac
}

main "$@"
