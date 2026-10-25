# Azure price catalog

The module `horizon_cloud::azure::catalog` reads Azure prices from the retail price
list. The module `horizon_cloud::offers::azure` makes Azure offers from these
prices. Both modules are separate from [the ARM client](azure-arm-client.md). They
use no credential and no subscription. `offers::AZURE_DEPLOYABLE` is `true`, so
the Azure section has `deployable: true`. Each Azure offer has `rentable: false`:
an offer is a price, not a place that this machine can use. Only the `azure`
settings decide the VM sizes and regions of a worker. New cloud lists the offers
that the section allows and places the cloud on the row that the person chooses.
`cloud_deploy --worker-choice` refuses an Azure offer, and a cloud without a
choice tries the sizes and regions of the section in their order. See
[the Azure provisioning architecture](azure-provisioning.md).

## Callers

- `cloud_runtime::prices::azure_catalog` fetches the catalog. It reads no
  settings and no credential.
- The agents' `cloud_offers` answer in Horizon fetches it on a background
  thread for a CPU request. The catalog stays current for one hour. A failed
  fetch is asked again after 30 seconds. The Azure section comes after the
  Hetzner and Docker host sections in `other_providers`.
- The CLI plan runner `cloud_deploy offers` fetches it for a CPU request.
- The New cloud dialog fetches it on a background thread when the machine has
  an `azure` section. It lists Azure rows only when the token of that section
  can be read. The catalog stays current for one hour.
  `offers::Workers::add_azure` keeps only the VM sizes and regions of the
  section. **Start cloud** stays off while the prices are older than one hour.
- A cloud worker does not receive it. A worker lists no Azure offers.

Azure offers alone do not replace a `RunPod` answer. If the `RunPod` fetch
failed and no other provider has a section, the answer is the `RunPod` error.

## Source

The catalog sends one GET request for each region in `REGIONS` to
`https://prices.azure.com/api/retail/prices`. The filter names the region, the
`Consumption` price type and three products:

- The VM products of the sizes in `VM_SIZES`: `DSv3`, `Ddsv4` and `FSv2`.
- `Premium SSD Managed Disks` in the `Storage` service.
- `IP Addresses` in the `Virtual Network` service.

The retail price list refuses a filter with many alternatives. Thus the filter
names products, not sizes. Each answer is one page of about 150 items.

The retail price list has no vCPU, memory or stock data. The curated table
`VM_SIZES` gives the vCPU and memory of each size. Creation confirms the stock,
so each offer has `availability: checked_at_creation`.

## Meters

The catalog uses an item only if it has `currencyCode: USD`, `type: Consumption`,
no `reservationTerm`, the requested `armRegionName` and a price of zero or more.
It does not read `isPrimaryMeterRegion`. That field is `false` for current
meters, for example the `DSv3` sizes and the static address.

| Price | Item fields |
|---|---|
| VM compute per hour | `serviceName: Virtual Machines`, `productName` equal to the product of the size, `armSkuName` equal to the size, `unitOfMeasure: 1 Hour`, and no `Spot` or `Low Priority` in `meterName` or `skuName` |
| Disk per month | `productName: Premium SSD Managed Disks`, `meterName: P<n> LRS Disk`, `unitOfMeasure: 1/Month` |
| Static address per hour | `productName: IP Addresses`, `skuName: Standard`, `meterName: Standard IPv4 Static Public IP`, `unitOfMeasure: 1 Hour` |

The exact product name removes the Windows, Cloud Services and Dedicated Host
prices. The exact disk meter name removes the `Disk Mount` meters of shared
disks. If two items for one meter give different prices, the catalog uses
neither.

## Estimate

Each offer is in US dollars. Azure has no monthly cap for a VM, so `monthly` is
absent.

| Field | Value |
|---|---|
| `hourly` | The compute price only |
| `estimated_total` | (compute + static address) × hours + (OS disk + workspace disk) × hours / 730 |
| `stopped_monthly` | OS disk + workspace disk + static address × 730 |

- The hours are a minimum of one and a maximum of one year.
- The OS disk is 30 GiB, or the default container disk if that is larger.
- The workspace disk is the requested storage, 20 GB by default.
- A disk gets the price of the smallest Premium SSD tier that holds it.
- A deallocated VM keeps both disks and the Standard static address. Thus the
  stopped cost includes them.

The estimate uses Premium SSD prices. If a later pull request uses a less
expensive disk type, the estimate is too high, not too low.

## Trust boundary

The answers of the retail price list are untrusted data.

The catalog guarantees these items:

- The production agent accepts HTTPS only, follows no redirect and stops each
  request after 30 seconds. It sends no credential.
- A body of 4 MiB or more gives `InvalidResponse`.
- A `NextPageLink` must start with the price endpoint, with or without port 443,
  and continue with a query. Any other link gives `InvalidResponse`. The catalog
  never sends a request to a host that an answer names.
- A repeated link, or more than 10 pages for one region, gives `InvalidResponse`.
- A page with a `BillingCurrency` that is not `USD` gives `InvalidResponse`.
- A transport failure, an HTTP failure or a cancellation in one region fails the
  full catalog.
- An item that the catalog cannot read prices nothing.
- A size without a compute price, a region without an address price, or a disk
  size without a tier price gives no offer. The catalog never estimates a
  missing price.

The catalog does not guarantee these items:

- It does not prove that a subscription can create a size in a region. Quota
  and stock are checked at creation.
- It does not apply discounts, reservations, savings plans or taxes.
- It does not include network egress, disk snapshots or other charges.
