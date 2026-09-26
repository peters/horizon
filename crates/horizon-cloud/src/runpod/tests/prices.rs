use super::*;

fn catalog_server(responses: Vec<(u16, String)>) -> (RunPod, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
    let (mut provider, requests, task) = server(responses);
    provider.catalog_endpoint.clone_from(&provider.endpoint);
    provider.graphql_endpoint.clone_from(&provider.endpoint);
    (provider, requests, task)
}

#[test]
fn secure_prices_and_the_best_gpu_availability_in_allowed_data_centers() {
    let cpus = json!({"cpus": [
        {"id": "cpu3c", "name": "Compute-Optimized", "price": {"securePerVcpu": 0.03}, "ramGbPerVcpu": 2},
        {"id": "cpu9x", "name": "Serverless only", "price": {"serverlessPerVcpu": 0.02}, "ramGbPerVcpu": 2},
        {"id": "cpu7x", "name": "Fractional memory", "price": {"securePerVcpu": 0.05}, "ramGbPerVcpu": 2.5}
    ]});
    let gpus = json!({"gpus": [
        {"id": "NVIDIA RTX A6000", "name": "RTX A6000", "memory": 48, "secure": true, "price": {"secure": 0.49, "community": 0.33}},
        {"id": "NVIDIA RTX 4000 Ada Generation", "name": "RTX 4000 Ada", "memory": 20, "secure": true, "price": {"secure": 0.28}},
        {"id": "NVIDIA A100-SXM4-40GB", "name": "A100 SXM 40GB", "memory": 40, "secure": false, "price": {"community": 1.0, "secure": 0}}
    ]});
    let centers = json!({"dataCenters": [
        {"id": "EU-RO-1", "region": "EUROPE", "networkVolumeTypes": ["STANDARD"], "gpuAvailability": [{"id": "NVIDIA RTX 4000 Ada Generation", "availability": "LOW"}]},
        {"id": "EU-SE-1", "region": "EUROPE", "networkVolumeTypes": [], "gpuAvailability": [{"id": "NVIDIA RTX 4000 Ada Generation", "availability": "HIGH"}]},
        {"id": "US-TX-3", "region": "NORTH_AMERICA", "gpuAvailability": [{"id": "NVIDIA RTX A6000", "availability": "HIGH"}]}
    ]});
    let (provider, requests, task) = catalog_server(vec![
        (200, cpus.to_string()),
        (200, gpus.to_string()),
        (200, centers.to_string()),
    ]);
    let list = provider
        .price_list(&["EU-RO-1".into(), "EU-SE-1".into()], &Cancellation::default())
        .unwrap();
    task.join().unwrap();
    assert_eq!(list.provider, "RunPod");
    let flavors: Vec<&str> = list.cpu.iter().map(|flavor| flavor.id.as_str()).collect();
    assert_eq!(flavors, ["cpu3c", "cpu7x"]);
    assert_eq!(list.gpus.len(), 2);
    let ada = list.gpu("NVIDIA RTX 4000 Ada Generation").unwrap();
    assert_eq!(ada.memory_gb, 20);
    assert_eq!(list.gpu_availability(&ada.id, &[]), crate::prices::Availability::High);
    assert_eq!(
        list.gpu_availability(&ada.id, &["EU-RO-1".into()]),
        crate::prices::Availability::Low
    );
    // The A6000 is only in stock outside the allowed data centers.
    assert_eq!(
        list.gpu_availability("NVIDIA RTX A6000", &[]),
        crate::prices::Availability::None
    );
    let centers: Vec<(&str, &str, bool)> = list
        .data_centers
        .iter()
        .map(|center| (center.id.as_str(), center.region.as_str(), center.workspace_storage))
        .collect();
    assert_eq!(centers, [("EU-RO-1", "EUROPE", true), ("EU-SE-1", "EUROPE", false)]);
    // Regions also name data centers outside the allowed set, where a worker may have
    // landed before the setting changed.
    assert_eq!(list.regions.get("US-TX-3").map(String::as_str), Some("NORTH_AMERICA"));
    assert_eq!(list.regions.len(), 3);
    let requests = requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /cpus "));
    assert!(requests[2].starts_with("GET /datacenters?include=GPU_AVAILABILITY "));
}

#[test]
fn an_unknown_gpu_availability_fails_the_price_list_instead_of_reading_as_sold_out() {
    let gpus = json!({"gpus": [
        {"id": "NVIDIA L4", "name": "L4", "memory": 24, "secure": true, "price": {"secure": 0.49}}
    ]});
    let catalog = |availability: &str| {
        vec![
            (200, json!({"cpus": []}).to_string()),
            (200, gpus.to_string()),
            (
                200,
                json!({"dataCenters": [
                    {"id": "EU-RO-1", "gpuAvailability": [{"id": "NVIDIA L4", "availability": availability}]}
                ]})
                .to_string(),
            ),
        ]
    };
    let (provider, _, task) = catalog_server(catalog("NONE"));
    let list = provider.price_list(&[], &Cancellation::default()).unwrap();
    task.join().unwrap();
    assert_eq!(
        list.gpu_availability("NVIDIA L4", &[]),
        crate::prices::Availability::None
    );

    let (provider, _, task) = catalog_server(catalog("SOMETIMES"));
    let result = provider.price_list(&[], &Cancellation::default());
    task.join().unwrap();
    assert!(matches!(result, Err(CloudError::InvalidResponse)));
}

#[test]
fn a_cpu_size_reports_its_best_stock_and_how_many_data_centers_have_it() {
    let mut profile = crate::CloudConfig::parse(crate::EXAMPLE).unwrap().profiles["image-only"].clone();
    profile.cpu = 8;
    profile.memory_gb = 16;
    profile.storage.container_gb = 20;
    let centers = json!({"dataCenters": [
        {"id": "EU-RO-1", "networkVolumeTypes": ["STANDARD"], "cpuAvailability": [{"id": "cpu3c", "availability": "HIGH"}]},
        {"id": "EU-SE-1", "networkVolumeTypes": ["STANDARD"], "cpuAvailability": [{"id": "cpu3c", "availability": "LOW"}]},
        {"id": "US-TX-3", "networkVolumeTypes": [], "cpuAvailability": [{"id": "cpu3c", "availability": "HIGH"}]}
    ]});
    let stock = json!({"data": {
        "s0": [{"id": "cpu3c", "specifics": {"stockStatus": "Low"}}],
        "s1": [{"id": "cpu3c", "specifics": {"stockStatus": "Medium"}}]
    }});
    let (provider, requests, task) = catalog_server(vec![(200, centers.to_string()), (200, stock.to_string())]);
    let size = provider
        .cpu_size_availability(&profile, &["cpu3c".into()], &[], &Cancellation::default())
        .unwrap();
    task.join().unwrap();
    assert_eq!(size.best(&[]), crate::prices::Availability::Medium);
    assert_eq!(size.count(&[]), 2);
    assert_eq!(size.best(&["EU-SE-1".into()]), crate::prices::Availability::Medium);
    let requests = requests.lock().unwrap();
    assert!(requests[1].contains("cpu3c-8-16"), "{}", requests[1]);
    // Data centers without standard network storage are never asked.
    assert!(!requests[1].contains("US-TX-3"));
}
