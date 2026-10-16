#!/bin/sh
# Runs only inside the pinned Go build stage; no Go tools enter the runtime image.
set -eu
export GOTOOLCHAIN=local CGO_ENABLED=0 GOMAXPROCS=6
export GOFLAGS=-mod=readonly
revision=5fb2a81b065b0a0bbbfc67ab20a0d9c6a1108115
archive_sha256=cae96823682d3ae1eaa2987ef31cf7b4bc06046933ccf1842cfd2aaf3094dba5
inputs=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
case "$(go env GOOS)/$(go env GOARCH)" in linux/amd64|linux/arm64) ;; *) echo 'unsupported Tailscale runtime architecture' >&2; exit 1;; esac
mkdir /tmp/tailscale-source
curl --fail --location --proto '=https' --tlsv1.2 --connect-timeout 20 --max-time 120 \
    "https://codeload.github.com/tailscale/tailscale/tar.gz/$revision" -o /tmp/tailscale-source.tar.gz
printf '%s  %s\n' "$archive_sha256" /tmp/tailscale-source.tar.gz | sha256sum -c -
tar -xzf /tmp/tailscale-source.tar.gz --strip-components=1 -C /tmp/tailscale-source
cd /tmp/tailscale-source
patch --batch --fuzz=0 -p1 < "$inputs/unix-proxy-v1.patch"
# Validate the checked-in scalar field/view/clone against the pinned generators.
sha256sum ipn/ipn_view.go ipn/ipn_clone.go > /tmp/ipn-generated.sha256
go generate ./ipn
sha256sum -c /tmp/ipn-generated.sha256
# Both native architecture jobs run the entire affected upstream test packages.
go test -p 6 -timeout 10m ./ipn ./ipn/ipnlocal ./cmd/tailscale/cli
patch_hash=$(sha256sum "$inputs/unix-proxy-v1.patch" | cut -d ' ' -f 1)
version="1.102.5-horizon-unix-proxy-v1-$patch_hash"
mkdir -p /output/usr/local/bin
for binary in tailscale tailscaled; do
    go build -p 6 -trimpath -buildvcs=false \
        -ldflags "-s -w -buildid= -X tailscale.com/version.longStamp=$version -X tailscale.com/version.shortStamp=$version -X tailscale.com/version.gitCommitStamp=$revision" \
        -o "/output/usr/local/bin/$binary" "./cmd/$binary"
done
go list -deps -json ./cmd/tailscale ./cmd/tailscaled > /tmp/tailscale-packages.json
python3 "$inputs/collect-notices.py" /tmp/tailscale-packages.json /output/usr/local/share/licenses/tailscale
install -m 644 "$inputs/unix-proxy-v1.patch" /output/usr/local/share/licenses/tailscale/
printf 'upstream=https://github.com/tailscale/tailscale/tree/%s\narchive_sha256=%s\npatch_sha256=%s\n' \
    "$revision" "$archive_sha256" "$patch_hash" > /output/usr/local/share/licenses/tailscale/SOURCE
go version > /output/usr/local/share/licenses/tailscale/GO-VERSION
go version -m /output/usr/local/bin/tailscale /output/usr/local/bin/tailscaled > /output/usr/local/share/licenses/tailscale/BUILD-INFO
chmod 755 /output/usr/local/bin/tailscale /output/usr/local/bin/tailscaled
