#!/usr/bin/env bash
# Registry steps of the Worker images workflow.
#   registry-tag.sh require-absent REF
#     Fails when REF exists, so a commit's tags are never published twice. Only a
#     "not found" or "manifest unknown" answer counts as absent; any other error fails.
#   registry-tag.sh digest REF
#     Prints the digest of the manifest or index that REF names: the SHA-256 of the
#     raw bytes the registry serves. A tag pushed a moment ago can take a few seconds
#     to appear, so it asks up to 12 times, 5 s apart.
set -euo pipefail

command=${1:?require-absent or digest}
ref=${2:?image reference}

case "$command" in
    require-absent)
        if output=$(docker buildx imagetools inspect --raw "$ref" 2>&1); then
            echo "$ref already exists; a published tag is never replaced" >&2
            exit 1
        fi
        if grep -qE '(: not found|manifest unknown)' <<< "$output"; then
            exit 0
        fi
        printf 'Cannot tell whether %s exists:\n%s\n' "$ref" "$output" >&2
        exit 1
        ;;
    digest)
        raw=$(mktemp)
        trap 'rm -f "$raw"' EXIT
        # $(...) would drop a final newline and change the digest, so the bytes go to a file.
        for attempt in $(seq 1 12); do
            if docker buildx imagetools inspect --raw "$ref" > "$raw" 2> /dev/null && [ -s "$raw" ]; then
                echo "sha256:$(sha256sum "$raw" | cut -d' ' -f1)"
                exit 0
            fi
            [ "$attempt" -lt 12 ] && sleep 5
        done
        echo "$ref did not appear in the registry" >&2
        exit 1
        ;;
    *)
        echo "unknown command: $command" >&2
        exit 2
        ;;
esac
