use super::*;
use crate::runpod::volumes::{Spec, State, Volume};
use std::{io::BufRead, sync::mpsc};

fn query_server(
    respond: impl Fn(&str) -> (u16, Value) + Send + 'static,
) -> (RunPod, mpsc::Sender<()>, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let (stop, stopped) = mpsc::channel();
    let task = thread::spawn(move || {
        let mut requests = Vec::new();
        while matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("fixture accept failed: {error}"),
            };
            // macOS copies the listener's nonblocking flag to accepted sockets, and a read
            // before the client's bytes arrive then fails with WouldBlock instead of waiting.
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut reader = std::io::BufReader::new(&mut stream);
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header == "\r\n" || header.is_empty() {
                    break;
                }
            }
            let (status, body) = respond(&request);
            requests.push(request);
            let body = body.to_string();
            write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        requests
    });
    let mut provider = RunPod::new(Credential::new("synthetic-key".into()).unwrap());
    provider.endpoint = format!("http://{address}");
    provider.api_endpoint.clone_from(&provider.endpoint);
    (provider, stop, task)
}

#[test]
fn serverless_attachment_prevents_owned_volume_deletion() {
    let worker_spec = spec();
    let volume_spec = Spec {
        operation_id: worker_spec.operation_id.clone(),
        size: u32::from(worker_spec.profile.storage.volume_gb),
        data_center_id: "test-region".into(),
    };
    let volume = Volume {
        id: "owned-volume".into(),
        name: volume_spec.name(),
        size: volume_spec.size,
        data_center_id: volume_spec.data_center_id.clone(),
    };
    let mut attached = worker(&worker_spec);
    attached["id"] = json!("serverless-worker");
    attached["networkVolume"] = json!({"id":volume.id,"size":volume.size,"dataCenterId":volume.data_center_id});
    let volume_body = serde_json::to_value(&volume).unwrap();
    let (provider, stop, task) = query_server(move |request| {
        if request.starts_with("GET /networkvolumes/owned-volume ") {
            (200, volume_body.clone())
        } else if request.starts_with("GET /pods") {
            (
                200,
                if request.contains("includeWorkers=true") {
                    let mut listed = attached.clone();
                    if !request.contains("includeNetworkVolume=true") {
                        listed["networkVolume"] = Value::Null;
                    }
                    json!([listed])
                } else {
                    json!([])
                },
            )
        } else {
            (404, json!({}))
        }
    });
    let mut state = State::Bound { volume, creation: None };
    let original = state.clone();
    let mut persisted = Vec::new();
    let mut reported = Vec::new();
    let result = provider.terminate_volume_with_progress(
        &volume_spec,
        &mut state,
        &Cancellation::default(),
        |next| {
            persisted.push(next.clone());
            Ok(())
        },
        |progress| reported.push(progress),
    );
    stop.send(()).unwrap();
    let requests = task.join().unwrap();
    assert!(matches!(
        result,
        Err(CloudError::Invalid(
            "Workspace volume is still attached to a worker; storage was not deleted"
        ))
    ));
    assert_eq!(state, original);
    assert!(persisted.is_empty());
    assert_eq!(
        reported,
        [Progress::ConfirmingVolume, Progress::CheckingAttachments],
        "refused storage never reports a deletion request"
    );
    assert!(requests.iter().all(|request| !request.starts_with("DELETE ")));
}

#[test]
fn fresh_inspection_exposes_unrequested_attachments_for_both_compute_profiles() {
    for gpu in [false, true] {
        let mut spec = spec();
        spec.profile.gpu = gpu;
        let mut assigned = worker(&spec);
        assigned["vcpuCount"] = json!(spec.profile.cpu);
        assigned["memoryInGb"] = json!(spec.profile.memory_gb);
        assigned["gpuCount"] = json!(1);
        assigned["containerDiskInGb"] = json!(spec.profile.storage.container_gb);
        assigned["volumeInGb"] = json!(spec.profile.storage.volume_gb);
        assigned["volumeMountPath"] = json!("/workspace");
        let (provider, stop, task) = query_server(move |request| {
            let mut value = assigned.clone();
            if request.contains("includeNetworkVolume=true") {
                value["networkVolume"] = json!({"id":"external-volume","size":80,"dataCenterId":"test-region"});
            }
            (200, value)
        });
        let result = provider.inspect_with_timeout("worker1", &Cancellation::default(), Duration::from_secs(2));
        stop.send(()).unwrap();
        let requests = task.join().unwrap();
        let inspected = result.unwrap().unwrap();
        inspected.verify(&spec).unwrap();
        assert!(
            inspected.verify_resources(&spec).is_err(),
            "unrequested attachment must block readiness, gpu={gpu}"
        );
        assert_eq!(requests.len(), 1);
    }
}
