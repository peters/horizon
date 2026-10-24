---
applyTo: "crates/horizon-cloud/src/azure/catalog.rs,crates/horizon-cloud/src/azure/catalog/**,crates/horizon-cloud/src/offers/azure.rs,crates/horizon-cloud/src/offers/azure/**"
---

# Azure price catalog boundary

Read `docs/architecture/azure-price-catalog.md` before review.

- A retail price list answer is untrusted data: status, body and `NextPageLink`.
- Send no credential. Keep the HTTPS-only, no-redirect and 30-second rules.
- Follow a `NextPageLink` only on the price endpoint, once, and at most 10 pages.
- A missing or disputed price drops the offer. Never estimate a missing meter.
- Keep `offers::AZURE_DEPLOYABLE` false: every Azure offer has `rentable: false`.
