---
procedure: azure-price-catalog
feature: The Azure VM size catalog, its retail prices and the Azure offers
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Azure price catalog test procedure

## 1. Purpose

This procedure makes sure that `horizon-cloud` reads Azure prices from the retail price list correctly.
It also makes sure that each Azure offer includes the disks and the static address, and that no single Azure offer is rentable.

## 2. Applicability

- Candidate: a pull request of #1774 that changes `horizon_cloud::azure::catalog` or `horizon_cloud::offers::azure`.
  It also applies to a change to the Azure part of a `cloud_offers` answer in the UI or the CLI plan runner.
- Platform: Linux with the repository Rust toolchain.
- The unit tests use synthetic price pages only. They need no network and no Azure account.
- Tasks AZP-04 and AZP-06 read the live retail price list. They need HTTPS access to `prices.azure.com` and no credential.
- The trust boundary of the catalog is in [the Azure price catalog architecture](../../architecture/azure-price-catalog.md).
- This procedure does not test these items:
  - The creation, stop, resume and delete of an Azure worker.
  - The Azure credential and the ARM client. [The ARM client procedure](azure-arm-client.md) tests the client.
  - The stock of a VM size. The retail price list has no stock data.
  - Azure in the New cloud dialog. Azure has no cloud settings yet, so the dialog shows no Azure offer.
  - Azure offers on a cloud worker. A cloud worker lists no Azure offers yet.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access.
Tasks AZP-04 and AZP-06 send GET requests without a credential to the public retail price list.
Task AZP-06 uses a temporary settings file that names no credential file that exists.

## 4. Equipment and preconditions

- A checkout of the candidate commit.
- The repository's supported Rust toolchain.
- A POSIX shell with `curl` and `jq`.
- HTTPS access to `prices.azure.com` for tasks AZP-04 and AZP-06.
- No Azure account, Azure credential or provider key.

## 5. Setup

1. Open a terminal in the candidate checkout.

   Result: `Cargo.toml` names the Horizon workspace.

2. Build the tests of `horizon-cloud`.

   ```sh
   cargo test -p horizon-cloud --lib --no-run
   ```

   Result: The build succeeds.

3. Make a temporary directory for the live price page.

   ```sh
   AZP_DIR="$(mktemp -d)"
   ```

   Result: `AZP_DIR` names a new empty directory.

## 6. Tasks

Do the tasks in order. Each task has a stable ID for the report.

### 6.1 AZP-01 — Read the retail price pages

1. Run the catalog tests.

   ```sh
   cargo test -p horizon-cloud --lib -- azure::catalog
   ```

   Result: 9 tests pass. These tests examine the items that follow:
   - The catalog uses only Linux pay-as-you-go meters in US dollars for the requested region.
   - The catalog ignores Windows, spot, low-priority, reservation, Cloud Services and Dedicated Host meters.
   - A size with two different prices, or with no price, gets no offer.
   - A size that is not in the curated table gets no offer.
   - A disk size gets the price of the smallest Premium SSD tier that holds it. A tier without a price gives no price.
   - The static address uses the Standard IPv4 static meter per hour.
   - The catalog follows `NextPageLink` one time for each page.
   - A link to a different host, a repeated link or more than 10 pages gives `InvalidResponse`.
   - A page in a different currency, a failed region or a cancellation fails the full catalog.

### 6.2 AZP-02 — Calculate the Azure offers

1. Run the offer tests.

   ```sh
   cargo test -p horizon-cloud --lib -- offers::azure
   ```

   Result: 6 tests pass. These tests examine the items that follow:
   - The estimated total is the compute and the static address for the hours, plus both disks for that time.
   - The OS disk is 30 GiB or the container disk, if that is larger. The workspace disk is the requested storage.
   - The stopped cost is both disks and the static address for one month.
   - An offer without a disk or address price is not shown.
   - The region, memory, vCPU and hourly price filters apply. A GPU request gets no Azure offer.
   - The answer section has `provider: Azure`, `currency: USD` and `deployable: true`. Each offer has `rentable: false`, because the `azure` settings decide the VM sizes and regions of a worker, not one offer.
   - The price uses a minimum of one hour and a maximum of one year.

### 6.3 AZP-03 — Make sure that the other offers do not change

1. Run the other offer tests.

   ```sh
   cargo test -p horizon-cloud --lib -- offers::
   ```

   Result: All tests pass. The Hetzner, `RunPod` and comparison results do not change.

2. Run the provider tests of `horizon-core`.

   ```sh
   cargo test -p horizon-core --lib -- cloud_runtime::providers
   ```

   Result: All tests pass. Without an `azure` section, each Azure lifecycle operation refuses and leaves the record unchanged.

### 6.4 AZP-04 — Examine the live retail price list

1. Get the northeurope page with the filter that the catalog sends.

   ```sh
   curl -sS -A horizon-procedure -o "$AZP_DIR/northeurope.json" "https://prices.azure.com/api/retail/prices?\$filter=priceType%20eq%20%27Consumption%27%20and%20armRegionName%20eq%20%27northeurope%27%20and%20((serviceName%20eq%20%27Virtual%20Machines%27%20and%20(productName%20eq%20%27Virtual%20Machines%20DSv3%20Series%27%20or%20productName%20eq%20%27Virtual%20Machines%20Ddsv4%20Series%27%20or%20productName%20eq%20%27Virtual%20Machines%20FSv2%20Series%27))%20or%20(serviceName%20eq%20%27Storage%27%20and%20productName%20eq%20%27Premium%20SSD%20Managed%20Disks%27)%20or%20(serviceName%20eq%20%27Virtual%20Network%27%20and%20productName%20eq%20%27IP%20Addresses%27))"
   ```

   Result: The command shows no error.

2. Examine the page envelope.

   ```sh
   jq -r '.BillingCurrency, (.NextPageLink // "no next page"), (.Items | length)' "$AZP_DIR/northeurope.json"
   ```

   Result: The output is `USD`, `no next page` and a number of items between 100 and 1000.

3. Examine the Linux pay-as-you-go meter of one size.

   ```sh
   jq -r '.Items[] | select(.productName == "Virtual Machines DSv3 Series" and .armSkuName == "Standard_D4s_v3" and (.meterName | test("Spot|Low Priority") | not)) | [.meterName, .retailPrice, .unitOfMeasure, .type, .isPrimaryMeterRegion] | @tsv' "$AZP_DIR/northeurope.json"
   ```

   Result: One line shows `D4s v3`, a price, `1 Hour` and `Consumption`. The last field can be `false`.

4. Examine the disk meters of the default OS disk and a 100 GiB workspace disk.

   ```sh
   jq -r '.Items[] | select(.meterName == "P4 LRS Disk" or .meterName == "P10 LRS Disk") | [.productName, .meterName, .retailPrice, .unitOfMeasure] | @tsv' "$AZP_DIR/northeurope.json"
   ```

   Result: Two lines show `Premium SSD Managed Disks`, a price and `1/Month`.

5. Examine the static address meter.

   ```sh
   jq -r '.Items[] | select(.meterName == "Standard IPv4 Static Public IP") | [.productName, .skuName, .retailPrice, .unitOfMeasure] | @tsv' "$AZP_DIR/northeurope.json"
   ```

   Result: One line shows `IP Addresses`, `Standard`, a price and `1 Hour`.

### 6.5 AZP-05 — Examine the Azure part of the agents' answer

1. Run the UI price tests for Azure.

   ```sh
   cargo test -p horizon-ui --bin horizon -- cloud_panel::production::prices::azure cloud_panel::production::offers
   ```

   Result: All tests pass. These tests examine the items that follow:
   - The `cloud_offers` answer shows the Azure section after the Hetzner section in `other_providers`.
   - Each Azure offer has `rentable: false`. The comparison shows its total in US dollars without an exchange rate.
   - A GPU request gets no Azure section.
   - If the `RunPod` fetch failed and only Azure has offers, the answer is the `RunPod` error.
   - A machine without a `RunPod` key shows the Azure offers beside the reason.
   - A failed, pending or refreshed Azure catalog is never shown as current.

### 6.6 AZP-06 — Rank the offers with the CLI plan runner

1. Write a settings file that names no `RunPod` key file that exists.

   ```sh
   printf '{"runpod_key_file":"%s/no-key","ssh_identity_file":"%s/no-identity","docker_config":"%s/no-docker","registry_pull_auth_id":null,"cpu_flavors":["cpu3c"],"gpu_types":[]}\n' "$AZP_DIR" "$AZP_DIR" "$AZP_DIR" > "$AZP_DIR/settings.json"
   ```

   Result: `$AZP_DIR/settings.json` exists.

2. Rank the CPU offers for 4 vCPU in northeurope for 10 hours.

   ```sh
   cargo run -q -p horizon-core --example cloud_deploy -- offers "$AZP_DIR/settings.json" '{"min_vcpu":4,"region":"northeurope","hours":10,"limit":3}' > "$AZP_DIR/offers.json"
   ```

   Result: The command shows no error.

3. Examine the Azure section.

   ```sh
   jq -r '.unavailable, (.other_providers[] | select(.provider == "Azure") | .currency, .deployable, (.offers[] | [.id, .location, .hourly, .estimated_total, .stopped_monthly, .rentable] | @tsv))' "$AZP_DIR/offers.json"
   ```

   Result: The output shows the `RunPod` reason, `USD` and `false`. Then 3 offers in `northeurope` show `false` in the last field.

4. Examine the comparison.

   ```sh
   jq -r '.comparison.complete, (.comparison.offers[0] | [.provider, .estimated_total_usd] | @tsv)' "$AZP_DIR/offers.json"
   ```

   Result: The output shows `true`, then `Azure` and the lowest estimated total of the Azure offers.

## 7. Pass criteria

- AZP-01, AZP-02, AZP-03 and AZP-05 pass.
- AZP-06 shows Azure offers in US dollars that are not rentable.
- AZP-04 shows the envelope fields and the meter names that the catalog reads.
- If AZP-04 shows a different field or meter name, the run fails. Record the new name in the report.

## 8. Cleanup

1. Remove the temporary directory.

   ```sh
   rm -r "$AZP_DIR"
   ```

   Result: The directory does not exist.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
