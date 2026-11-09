---
applyTo: "crates/horizon-cloud/src/gcp.rs,crates/horizon-cloud/src/gcp/**"
---

# Google Cloud service-account key boundary

Read `crates/horizon-cloud/src/gcp/token.rs` and the Google Cloud epic #2003.
This code turns a service-account key into an access token. It makes no Compute
Engine call yet.

## Untrusted input

- The service-account key document. A person chooses the file, but its content is
  untrusted: Horizon does not know who wrote it. Every field is attacker-chosen if
  the file is planted, and `token_uri` names the address the signed assertion is
  sent to.
- The answer of the token endpoint: its status, its headers and its body. The body
  decides the token text and the lifetime that Horizon then trusts.
- The system clock, which decides `iat`, `exp` and when a token is renewed.

## What this code guarantees

- A document that is not a service-account key is refused before any key is read.
  `client_email`, `project_id`, `token_uri` and `private_key` are each checked, and
  the error names the field that failed, never its value.
- The assertion travels to a Google token host only. `token_uri` must be
  `https://oauth2.googleapis.com` or `https://accounts.google.com`, matched on the
  whole authority, so neither a suffix such as
  `oauth2.googleapis.com.example`, nor a subdomain such as
  `evil.oauth2.googleapis.com`, nor an explicit port passes. Clear text reaches a
  loopback address only, which is what a test binds. A planted document therefore
  cannot make Horizon send a key-signed artifact to a host of its choosing.
- `aud` is the `token_uri`, so an assertion is bound to the endpoint it was made
  for and is not replayable elsewhere.
- The private key never appears in an error, in `Debug` or in a log. It is held in
  `Zeroizing`, and so is the exchange body that carries the assertion.
- A refused exchange reports the status and the provider's error code only. The
  answer text is never echoed, so a provider cannot place a secret in a Horizon
  error.
- A token is renewed five minutes before it expires, so a request does not carry a
  token that is about to lapse. A renewal that fails leaves the cache able to try
  again rather than serving a stale token.
- An answer that is not a token, or a token that is not printable, is a typed error
  rather than a value that reaches a request.

## What this code does not guarantee

- It does not prove the key belongs to the person who chose the file. A planted key
  makes Horizon act as that service account, within the pinned destination above.
  Nothing here limits what the account itself may do in its project; the project's
  own IAM roles do that.
- It does not check the key against a revocation list. A revoked key fails at the
  token endpoint, not before it.
- It does not support a Google universe other than `googleapis.com`. A key for
  another universe is refused by the pinned host, and the error names `token_uri`.
- It does not pin the TLS certificate of the token host. Transport security is
  whatever `ureq` with `rustls` provides.
- It does not read the ambient environment. There is no metadata-server path and no
  `GOOGLE_APPLICATION_CREDENTIALS` path, so a token comes from the chosen document
  only.
