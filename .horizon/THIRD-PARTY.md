# Worker image components

The image is a collection of separately licensed components. The Horizon MIT
license does not replace their licenses. Preserve the supplied licenses and
copyright notices when redistributing the image.

- Horizon helper binaries and worker scripts are built from the public revision
  recorded in `/usr/local/share/licenses/horizon/SOURCE`. Their MIT license and
  collected dependency notices, including nested vendored notices and package
  provenance, are under `/usr/local/share/licenses/`.
- Rust and its tools retain their installed documentation and licenses under
  `/usr/local/rustup/toolchains/`. Node retains its license and third-party
  notices under `/usr/local/share/licenses/node/`; package licenses remain in
  `/usr/local/lib/node_modules/`.
- The unmodified agent clients are pinned in the installer. The Apache-2.0
  client retains its package license. Preinstalling the other client requires
  the applicable Commercial Terms, an unmodified binary, unrestricted built-in
  authentication, and each end user's own account and billing. See
  https://code.claude.com/docs/en/legal-and-compliance . No credentials are
  supplied with these images. Supplemental bundled-client notices are retained
  under `/usr/local/share/licenses/agent-client/`. Exact source archives for the
  bundled sandbox helper and native voice libraries, including the corresponding
  build scripts, are under `/usr/local/share/sources/agent-components/`. Its
  manifest records immutable source identities and verified archive checksums.
  Preserve these archives with the binaries; the voice libraries remain separate
  replaceable shared objects. Other component terms continue to apply.
- Chromium, its common files and sandbox, libdav1d6 and libjpeg62-turbo come
  from the signed Debian package repositories. The exact versions are pinned
  in the recipes; their copyright and source information remain under
  `/usr/share/doc/<package>/copyright`. Source packages and distribution metadata
  are available at https://sources.debian.org/src/chromium/ ,
  https://sources.debian.org/src/dav1d/ and
  https://sources.debian.org/src/libjpeg-turbo/ . All other operating-system
  dependencies come from the configured Ubuntu repositories. No Debian apt
  repository is added to the running worker.
- Firefox is the unmodified distribution from Mozilla's signed repository. Keep
  its bundled licenses, branding and distribution-policy requirements:
  https://www.mozilla.org/foundation/trademarks/distribution-policy/ .
- Geckodriver 0.36.0 is unmodified, with MPL-2.0 license and corresponding source
  at https://github.com/mozilla/geckodriver/tree/v0.36.0 . Its license is retained
  under `/usr/local/share/licenses/geckodriver/`.
- The GPU image derives directly from the complete official CUDA development
  container. Its container license, CUDA EULA and package notices remain in
  place. The container's distribution grant and restrictions, including the
  supported GPU platform and downstream terms, continue to apply:
  https://developer.nvidia.com/ngc/nvidia-deep-learning-container-license .
  No separate inference-runtime or neural-network-library layer is added.

The recipes do not delete operating-system copyright documentation. Image
publication must retain this file, the component notices, and immutable source
and package identities. Runtime repository checkouts, package-feed credentials,
agent login state, SSH keys and application data must never enter image layers.
