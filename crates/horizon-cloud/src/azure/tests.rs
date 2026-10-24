use super::*;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use ureq::{
    Body, SendBody,
    http::{Request, Response},
    middleware::MiddlewareNext,
};

mod reconcile;
mod transport;

const SUB: &str = "00000000-0000-4000-8000-000000000001";
const OTHER_SUB: &str = "00000000-0000-4000-8000-000000000002";
const TOKEN: &str = "synthetic-arm-token";
const OPERATION_ID: &str = "op-1";
const GROUP: &str = "horizon-ws-op-1";
const VM_INSTANCE: &str = "3f2c9a1e-5d4b-4c6a-8e7f-0a1b2c3d4e5f";

/// One scripted exchange: what the client must send and what ARM answers.
struct Exchange {
    method: &'static str,
    url: String,
    request_body: Option<Value>,
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn get(url: String, status: u16, body: impl Into<String>) -> Exchange {
    Exchange {
        method: "GET",
        url,
        request_body: None,
        status,
        headers: Vec::new(),
        body: body.into(),
    }
}

impl Exchange {
    fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
}

/// The client under test and the `METHOD url` of every request it sent.
struct Scripted {
    azure: Azure,
    sent: Arc<Mutex<Vec<String>>>,
    queue: Arc<Mutex<VecDeque<Exchange>>>,
}

impl Scripted {
    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }

    /// Every scripted exchange was used.
    fn done(&self) {
        assert!(self.queue.lock().unwrap().is_empty(), "unused scripted answers");
    }
}

/// A client whose agent answers from `script` in middleware. Each request must match
/// the next exchange's method, absolute URL, bearer header and JSON body.
fn scripted(script: Vec<Exchange>) -> Scripted {
    let queue = Arc::new(Mutex::new(VecDeque::from(script)));
    let sent = Arc::new(Mutex::new(Vec::new()));
    let (answers, log) = (queue.clone(), sent.clone());
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .middleware(move |request: Request<SendBody>, _: MiddlewareNext| {
            let line = format!("{} {}", request.method(), request.uri());
            log.lock().unwrap().push(line.clone());
            let exchange = answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("unscripted request {line}"));
            assert_eq!(request.method().as_str(), exchange.method, "{line}");
            assert_eq!(request.uri().to_string(), exchange.url, "{line}");
            assert_eq!(request.headers()["Authorization"], format!("Bearer {TOKEN}"));
            let mut payload = String::new();
            request.into_body().into_reader().read_to_string(&mut payload).unwrap();
            match exchange.request_body {
                Some(expected) => assert_eq!(serde_json::from_str::<Value>(&payload).unwrap(), expected),
                None => assert!(payload.is_empty(), "unexpected body on {line}"),
            }
            let mut response = Response::builder().status(exchange.status);
            for (name, value) in exchange.headers {
                response = response.header(name, value);
            }
            Ok(response.body(Body::builder().data(exchange.body)).unwrap())
        })
        .build()
        .new_agent();
    let azure = Azure::with_agent(agent, Credential::new(TOKEN.into()).unwrap(), SUB).unwrap();
    Scripted { azure, sent, queue }
}

fn group_url(name: &str) -> String {
    format!("https://management.azure.com/subscriptions/{SUB}/resourceGroups/{name}?api-version=2022-09-01")
}

fn vm_url(group: &str, name: &str) -> String {
    format!(
        "https://management.azure.com/subscriptions/{SUB}/resourceGroups/{group}/providers/Microsoft.Compute/virtualMachines/{name}?api-version=2024-03-01&$expand=instanceView"
    )
}

fn group_body(name: &str, operation: Option<&str>) -> String {
    let tags = operation.map_or_else(|| json!({}), |operation| json!({ OPERATION_TAG: operation, "n": 1 }));
    json!({
        "id": format!("/subscriptions/{SUB}/resourceGroups/{name}"),
        "name": name,
        "location": "northeurope",
        "properties": { "provisioningState": "Succeeded" },
        "tags": tags,
    })
    .to_string()
}

fn vm_body(name: &str, instance: Value) -> String {
    let mut body = json!({
        "id": format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/virtualMachines/{name}"),
        "name": name,
        "location": "northeurope",
        "tags": { OPERATION_TAG: OPERATION_ID },
        "properties": {
            "provisioningState": "Succeeded",
            "hardwareProfile": { "vmSize": "Standard_D4s_v5" },
            "storageProfile": { "dataDisks": [
                { "lun": 0, "deleteOption": "Detach", "managedDisk": {
                    "id": format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/disks/worker-data") } },
                { "lun": 1, "managedDisk": {} }
            ] },
            "instanceView": { "statuses": [
                { "code": "ProvisioningState/succeeded" }, { "code": "PowerState/running" }
            ] }
        }
    });
    if !instance.is_null() {
        body["properties"]["vmId"] = instance;
    }
    body.to_string()
}

fn arm_error(code: &str, message: &str) -> String {
    json!({ "error": { "code": code, "message": message } }).to_string()
}
