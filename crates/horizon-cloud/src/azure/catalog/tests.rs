use super::*;
use serde_json::{Value, json};

fn item(service: &str, product: &str, sku: &str, meter: &str, unit: &str, price: f64) -> Value {
    json!({
        "currencyCode": "USD",
        "retailPrice": price,
        "unitPrice": price,
        "armRegionName": "northeurope",
        "location": "EU North",
        "meterName": meter,
        "productName": product,
        "skuName": sku,
        "serviceName": service,
        "unitOfMeasure": unit,
        "type": "Consumption",
        "isPrimaryMeterRegion": false,
        "armSkuName": "",
    })
}

fn vm(arm_sku: &str, product: &str, meter: &str, price: f64) -> Value {
    let mut value = item("Virtual Machines", product, meter, meter, "1 Hour", price);
    value["armSkuName"] = json!(arm_sku);
    value
}

fn disk(tier: &str, price: f64) -> Value {
    item(
        "Storage",
        DISK_PRODUCT,
        &format!("{tier} LRS"),
        &format!("{tier} LRS Disk"),
        "1/Month",
        price,
    )
}

fn address(price: f64) -> Value {
    item(
        "Virtual Network",
        ADDRESS_PRODUCT,
        "Standard",
        ADDRESS_METER,
        "1 Hour",
        price,
    )
}

fn page(items: &[Value], next: Option<&str>) -> Vec<u8> {
    json!({
        "BillingCurrency": "USD",
        "CustomerEntityId": "Default",
        "CustomerEntityType": "Retail",
        "Items": items,
        "NextPageLink": next,
        "Count": 0,
    })
    .to_string()
    .into_bytes()
}

/// A catalog whose `northeurope` answer is `items` and whose other regions answer empty.
fn northeurope(items: Vec<Value>) -> Catalog {
    let mut items = Some(items);
    let fetched = Catalog::fetch_with(
        |url| {
            Ok(if url.contains("armRegionName%20eq%20%27northeurope%27") {
                page(&items.take().unwrap_or_default(), None)
            } else {
                page(&[], None)
            })
        },
        &Cancellation::default(),
    );
    match fetched {
        Ok(catalog) => catalog,
        Err(error) => panic!("synthetic catalog: {error}"),
    }
}

#[test]
fn retail_prices_are_read_in_usd_for_linux_pay_as_you_go_meters() {
    let mut reserved = vm("Standard_D4s_v3", DSV3, "D4s v3", 0.1);
    reserved["type"] = json!("Reservation");
    reserved["reservationTerm"] = json!("1 Year");
    let mut euros = vm("Standard_F4s_v2", FSV2, "F4s v2", 0.1);
    euros["currencyCode"] = json!("EUR");
    let mut elsewhere = vm("Standard_F2s_v2", FSV2, "F2s v2", 0.05);
    elsewhere["armRegionName"] = json!("westeurope");
    let catalog = northeurope(vec![
        vm("Standard_D4s_v3", DSV3, "D4s v3", 0.214),
        vm("Standard_D4s_v3", &format!("{DSV3} Windows"), "D4s v3", 0.398),
        vm("Standard_D4s_v3", DSV3, "D4s v3 Spot", 0.039),
        vm("Standard_D4s_v3", DSV3, "D4s v3 Low Priority", 0.042),
        vm("Standard_D4s_v3", "DSv3 Series Dedicated Host", "D4s v3", 0.5),
        reserved,
        euros,
        elsewhere,
        vm("Standard_D4ds_v4", DDSV4, "D4ds v4", -1.0),
        json!({"armSkuName": "Standard_D8s_v3", "retailPrice": "0.428"}),
    ]);
    assert_eq!(
        catalog.offers,
        vec![Offer {
            size: "Standard_D4s_v3".into(),
            region: "northeurope".into(),
            vcpu: 4,
            memory_gb: 16,
            hourly_usd: 0.214,
        }]
    );
}

#[test]
fn a_size_with_two_different_linux_prices_is_left_out() {
    let catalog = northeurope(vec![
        vm("Standard_D2s_v3", DSV3, "D2s v3", 0.107),
        vm("Standard_D2s_v3", DSV3, "D2s v3", 0.2),
        vm("Standard_F2s_v2", FSV2, "F2s v2", 0.096),
        vm("Standard_F2s_v2", FSV2, "F2s v2", 0.096),
    ]);
    let sizes: Vec<_> = catalog.offers.iter().map(|offer| offer.size.as_str()).collect();
    assert_eq!(sizes, ["Standard_F2s_v2"]);
}

#[test]
fn the_vm_size_table_covers_the_priced_sizes_and_skips_an_unknown_meter() {
    let catalog = northeurope(vec![
        vm("Standard_D4s_v3", DSV3, "D4s v3", 0.214),
        vm("Standard_D48s_v3", DSV3, "D48s v3", 2.568),
    ]);
    assert_eq!(catalog.offers.len(), 1);
    for size in VM_SIZES {
        assert!(size.vcpu > 0 && size.memory_gb >= size.vcpu, "{}", size.name);
    }
}

#[test]
fn disk_meters_build_a_tier_table_and_a_size_rounds_up() {
    let mut mount = disk("P6", 0.564);
    mount["meterName"] = json!("P6 LRS Disk Mount");
    let mut hdd = disk("P10", 1.0);
    hdd["productName"] = json!("Standard HDD Managed Disks");
    let catalog = northeurope(vec![
        disk("P4", 5.28),
        disk("P6", 10.21),
        mount,
        hdd,
        disk("P70", 1802.06),
    ]);
    assert_eq!(catalog.disk_month("northeurope", 30), Some(5.28));
    assert_eq!(catalog.disk_month("northeurope", 32), Some(5.28));
    assert_eq!(catalog.disk_month("northeurope", 33), Some(10.21));
    assert_eq!(catalog.disk_month("northeurope", 10_240), Some(1802.06));
    // P10 has no Premium meter, so a 100 GiB disk is unpriced, never billed as P15.
    assert_eq!(catalog.disk_month("northeurope", 100), None);
    assert_eq!(catalog.disk_month("westeurope", 30), None);
    assert_eq!(tier_gb(32_767), Some(32_767));
    assert_eq!(tier_gb(32_768), None);
}

#[test]
fn the_static_address_is_the_standard_hourly_meter() {
    let mut basic = address(0.0036);
    basic["skuName"] = json!("Basic");
    basic["meterName"] = json!("Basic IPv4 Static Public IP");
    let catalog = northeurope(vec![basic, address(0.005)]);
    assert_eq!(
        catalog.static_ip_hour_usd,
        BTreeMap::from([("northeurope".into(), 0.005)])
    );
    assert!(northeurope(Vec::new()).static_ip_hour_usd.is_empty());
}

#[test]
fn region_labels_read_as_other_providers_name_them() {
    let catalog = northeurope(Vec::new());
    assert_eq!(catalog.regions.len(), REGIONS.len());
    assert_eq!(catalog.regions["northeurope"], "EUROPE");
    assert_eq!(catalog.regions["eastus"], "NORTH_AMERICA");
    assert_eq!(catalog.regions["japaneast"], "ASIA");
    assert_eq!(catalog.regions["australiaeast"], "OCEANIA");
}

#[test]
fn every_page_of_the_retail_price_list_is_followed_once() {
    let second = format!("{ENDPOINT_WITH_PORT}?$filter=x&$skip=1000");
    let mut asked = Vec::new();
    let mut answers = vec![
        page(&[vm("Standard_D4s_v3", DSV3, "D4s v3", 0.214)], Some(&second)),
        page(&[address(0.005)], None),
    ]
    .into_iter();
    let items = pages(
        &mut |url: &str| {
            asked.push(url.to_owned());
            answers.next().ok_or(CloudError::Transport)
        },
        first_page("northeurope"),
        &Cancellation::default(),
    );
    assert_eq!(items.map(|items| items.len()).ok(), Some(2));
    assert_eq!(asked.len(), 2);
    assert!(asked[0].starts_with(&format!("{ENDPOINT}?$filter=priceType%20eq%20%27Consumption%27")));
    assert!(asked[0].contains("productName%20eq%20%27Virtual%20Machines%20DSv3%20Series%27"));
    assert!(!asked[0].contains(' ') && !asked[0].contains('\''));
    assert_eq!(asked[1], format!("{ENDPOINT}?$filter=x&$skip=1000"));
}

#[test]
fn a_repeated_foreign_or_endless_page_link_is_refused() {
    let first = first_page("northeurope");
    let fetch = |next: String| {
        pages(
            &mut |_: &str| Ok(page(&[], Some(&next))),
            first.clone(),
            &Cancellation::default(),
        )
        .err()
    };
    for link in [
        first.clone(),
        "https://prices.example.com/api/retail/prices?$skip=1".to_owned(),
        "https://prices.azure.com/api/retail/prices.example.com?$skip=1".to_owned(),
        "http://prices.azure.com/api/retail/prices?$skip=1".to_owned(),
    ] {
        assert!(
            matches!(fetch(link.clone()), Some(CloudError::InvalidResponse)),
            "{link}"
        );
    }
    let mut count = 0;
    let endless = pages(
        &mut |_: &str| {
            count += 1;
            Ok(page(&[], Some(&format!("{ENDPOINT}?$skip={count}"))))
        },
        first,
        &Cancellation::default(),
    );
    assert!(matches!(endless, Err(CloudError::InvalidResponse)));
    assert_eq!(count, MAX_PAGES);
}

#[test]
fn a_page_in_another_currency_or_a_failed_region_fails_the_catalog() {
    let mut euros = json!({"BillingCurrency": "EUR", "Items": [], "NextPageLink": null});
    let answer = |body: Value| Catalog::fetch_with(|_| Ok(body.to_string().into_bytes()), &Cancellation::default());
    assert!(matches!(answer(euros.take()), Err(CloudError::InvalidResponse)));
    assert!(matches!(answer(json!({"Items": []})), Err(CloudError::InvalidResponse)));
    let failed = Catalog::fetch_with(
        |url| {
            if url.contains("japaneast") {
                Err(CloudError::Http(503, Reason::default()))
            } else {
                Ok(page(&[], None))
            }
        },
        &Cancellation::default(),
    );
    assert!(matches!(failed, Err(CloudError::Http(503, _))));
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        Catalog::fetch_with(|_| Ok(page(&[], None)), &cancel),
        Err(CloudError::Cancelled)
    ));
}
