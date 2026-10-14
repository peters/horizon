use super::*;

#[test]
fn extra_names_are_lowercase_host_names_or_wildcards_over_a_registered_name() {
    let egress = |names: &[&str]| Egress {
        allow: names.iter().map(|name| (*name).to_owned()).collect(),
    };
    assert!(
        egress(&["nuget.example.com", "*.test.example.no", "a-1.example"])
            .validate()
            .is_ok()
    );
    for invalid in [
        "*.com",
        "localhost",
        "Example.com",
        "example.com.",
        "*example.com",
        "a.*.example.com",
        "-a.example.com",
        "a_b.example.com",
        "10.0.0.1",
        "1.2.3.4.5",
        "example.com:443",
        "https://example.com",
        "",
    ] {
        assert!(egress(&[invalid]).validate().is_err(), "{invalid}");
    }
    let many: Vec<String> = (0..=ALLOW_LIMIT).map(|n| format!("n{n}.example.com")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    assert!(egress(&many).validate().is_err());
    assert!(egress(&many[1..]).validate().is_ok());
}

#[test]
fn the_allowlist_file_has_the_defaults_and_the_extra_names_one_a_line() {
    let egress = Egress {
        allow: ["*.test.example.no".to_owned()].into(),
    };
    let file = egress.allowlist();
    let lines: Vec<&str> = file.lines().collect();
    assert_eq!(
        lines.len(),
        DEFAULT_ALLOW.len() + usize::from(ACTIONS_RESULTS_ACCOUNTS) + 1
    );
    assert!(lines.contains(&"productionresultssa0.blob.core.windows.net"));
    assert!(lines.contains(&"productionresultssa19.blob.core.windows.net"));
    assert!(
        !lines
            .iter()
            .any(|line| line.starts_with("*.") && line.ends_with("windows.net"))
    );
    assert_eq!(lines.last(), Some(&"*.test.example.no"));
    assert!(file.ends_with('\n'));
    for name in DEFAULT_ALLOW {
        assert!(valid_entry(name), "{name}");
        assert!(!name.starts_with("*."), "the defaults are exact names: {name}");
    }
    // No default lets the agent publish where anyone can read it, or sends a release.
    for refused in [
        "uploads.github.com",
        "gist.github.com",
        "crates.io",
        "www.nuget.org",
        "upload.pypi.org",
    ] {
        assert!(!DEFAULT_ALLOW.contains(&refused), "{refused}");
    }
}

#[test]
fn a_profile_without_egress_is_saved_as_before() {
    let saved: Egress = serde_json::from_str("{}").unwrap();
    assert!(saved.allow.is_empty());
    assert_eq!(serde_json::to_string(&saved).unwrap(), "{}");
    assert!(serde_json::from_str::<Egress>(r#"{"deny":[]}"#).is_err());
}

/// Runs `test` after the definitions of the host's egress script, without its entry point.
/// A test that hangs, for example a relay that never ends, fails after 30 seconds. The
/// script runs on a Linux host, so these tests run on Unix only.
#[cfg(unix)]
fn python(test: &str) {
    let script = include_str!("../host/egress.py");
    let definitions = script.split("if __name__ == '__main__':").next().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let output = std::process::Command::new("python3")
        .args([
            "-I",
            "-c",
            &format!("{definitions}\nimport signal\nsignal.alarm(30)\n{test}"),
        ])
        .current_dir(folder.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
#[cfg(unix)]
fn the_host_allows_exact_names_and_names_under_a_wildcard_only() {
    python(
        r"
allow = ({'github.com'}, {'test.example.no'})
for name in ('github.com', 'GitHub.com', 'github.com.', 'pr-1.test.example.no', 'a.b.test.example.no'):
    assert allowed(allow, name), name
for name in ('api.github.com', 'evilgithub.com', 'github.com.evil.example', 'test.example.no',
             'xtest.example.no', 'example.com', '', '.', 'github..com', 'git hub.com', '1.2.3.4x',
             'github.com\x00', 'gïthub.com', 'a' * 64 + '.github.com'):
    assert not allowed(allow, name), name
with open('allow', 'w') as file:
    file.write('GitHub.com\n\n*.Test.example.no\n')
assert load('allow') == ({'github.com'}, {'test.example.no'})
",
    );
}

/// A TLS `ClientHello` in Python with `name` in its server name extension, or none.
#[cfg(unix)]
const HELLO: &str = r"
def hello(name=None, padding=0):
    extensions = b''
    if name is not None:
        entry = b'\x00' + len(name).to_bytes(2, 'big') + name
        listed = len(entry).to_bytes(2, 'big') + entry
        extensions += b'\x00\x00' + len(listed).to_bytes(2, 'big') + listed
    extensions += b'\x00\x15' + padding.to_bytes(2, 'big') + bytes(padding)
    body = (b'\x03\x03' + bytes(32) + b'\x00' + b'\x00\x02\x13\x01' + b'\x01\x00'
            + len(extensions).to_bytes(2, 'big') + extensions)
    return b'\x01' + len(body).to_bytes(3, 'big') + body

def records(message, size=16384):
    out = b''
    for at in range(0, len(message), size):
        part = message[at:at + size]
        out += b'\x16\x03\x01' + len(part).to_bytes(2, 'big') + part
    return out
";

#[test]
#[cfg(unix)]
fn the_proxy_reads_the_name_of_a_client_hello_across_records_and_refuses_anything_else() {
    python(&format!(
        r"{HELLO}
assert server_name(hello(b'github.com')) == 'github.com'
assert server_name(hello()) is None
assert server_name(hello(b'github.com')[:-3]) is None
assert server_name(b'') is None
assert server_name(b'\x02' + hello(b'github.com')[1:]) is None
# A name that reaches past its extension, or a list length that does not match, is refused.
good = hello(b'github.com')
at = good.index(b'\x00\x00\x00\x0f')
assert server_name(good[:at + 2] + b'\x00\x40' + good[at + 4:]) is None
assert server_name(good[:at + 4] + b'\x00\x0e' + good[at + 6:]) is None
# A client that sends one byte at a time gets no more time than the whole budget.
HELLO_SECONDS = 0.5
left, right = socket.socketpair()
def trickle():
    try:
        for byte in records(hello(b'github.com')):
            left.sendall(bytes([byte]))
            time.sleep(0.05)
    except OSError:
        pass
threading.Thread(target=trickle, daemon=True).start()
started = time.monotonic()
try:
    assert read_hello(right)[1] is None
except TimeoutError:
    pass
assert time.monotonic() - started < 2
left.close(); right.close()
HELLO_SECONDS = 10
# A complete ClientHello larger than the budget is never read in full.
left, right = socket.socketpair()
big = records(hello(b'github.com', padding=HELLO_LIMIT))
threading.Thread(target=lambda: (left.sendall(big), left.close())).start()
read, handshake = read_hello(right)
assert handshake is None and len(read) <= HELLO_LIMIT, len(read)
right.close()
message = hello(b'api.github.com', padding=3000)
for size, chunk in ((16384, 4096), (1000, 7), (5, 1)):
    left, right = socket.socketpair()
    data = records(message, size)
    def send(data=data, chunk=chunk):
        for at in range(0, len(data), chunk):
            left.sendall(data[at:at + chunk])
        left.sendall(b'after')
    threading.Thread(target=send).start()
    read, handshake = read_hello(right)
    assert handshake == message, size
    assert server_name(handshake) == 'api.github.com'
    assert data.startswith(read[:len(data)]) or read.startswith(data)
    left.close(); right.close()
for junk in (b'GET / HTTP/1.1\r\nHost: github.com\r\n\r\n', b'\x17\x03\x03\x00\x05hello',
             records(b'\x01\xff\xff\xff' + bytes(20000))):
    left, right = socket.socketpair()
    threading.Thread(target=lambda junk=junk: (left.sendall(junk), left.close())).start()
    assert read_hello(right)[1] is None, junk[:8]
    right.close()
"
    ));
}

#[test]
#[cfg(unix)]
fn the_proxy_connects_only_to_public_addresses_of_an_allowed_name_and_relays_both_ways() {
    python(&format!(
        r"{HELLO}
def resolve(name, port, type):
    return [(socket.AF_INET, socket.SOCK_STREAM, 6, '', (address, port)) for address in
            ('127.0.0.1', '169.254.169.254', '10.0.0.1', '172.30.0.1', '192.0.2.1', '8.8.8.8')]
assert [found[3][0] for found in global_addresses('github.com', resolve)] == ['8.8.8.8']

class Recorder:
    def __init__(self): self.names = []
    def refused(self, name, via): self.names.append((name, via))

allow = ({{'github.com'}}, set())
opened = []
def opener(name):
    near, far = socket.socketpair()
    opened.append((name, far))
    return near

for name in (b'example.com', None):
    log = Recorder()
    client, proxy = socket.socketpair()
    client.sendall(records(hello(name)))
    relay(proxy, allow, log, opener)
    assert log.names == [((name or b'-').decode(), 'tls')], log.names
    assert client.recv(10) == b''
assert opened == []

log = Recorder()
client, proxy = socket.socketpair()
sent = records(hello(b'GitHub.com'))
client.sendall(sent + b'early data')
thread = threading.Thread(target=relay, args=(proxy, allow, log, opener))
thread.start()
while not opened:
    time.sleep(0.01)
name, server = opened[0]
assert name == 'GitHub.com'
received = b''
while len(received) < len(sent) + 10:
    received += server.recv(65536)
assert received == sent + b'early data'
server.sendall(b'reply')
assert client.recv(5) == b'reply'
client.shutdown(socket.SHUT_WR)
assert server.recv(10) == b''
server.close()
thread.join(5)
assert not thread.is_alive() and log.names == []

# A silent client keeps a connection while the answer streams; both quiet ends it.
IDLE_SECONDS, IDLE_CHECK = 1, 0.2
opened.clear()
client, proxy = socket.socketpair()
client.sendall(records(hello(b'github.com')))
thread = threading.Thread(target=relay, args=(proxy, allow, log, opener))
thread.start()
while not opened:
    time.sleep(0.01)
server = opened[0][1]
server.recv(65536)
for _ in range(8):
    server.sendall(b'x')
    time.sleep(0.3)
received = b''
client.settimeout(5)
while len(received) < 8:
    received += client.recv(10)
assert received == b'x' * 8 and thread.is_alive()
thread.join(5)
assert not thread.is_alive()
assert client.recv(10) == b''
"
    ));
}

#[test]
#[cfg(unix)]
fn the_resolver_answers_allowed_names_only_and_refuses_the_rest_with_nxdomain() {
    python(
        r"
def query(name, ident=b'\x12\x34', qtype=1):
    encoded = b''.join(bytes([len(part)]) + part for part in name.split(b'.') if part) + b'\x00'
    return ident + b'\x01\x20' + b'\x00\x01\x00\x00\x00\x00\x00\x01' + encoded + qtype.to_bytes(2, 'big') + b'\x00\x01' \
        + b'\x00\x00\x29\x10\x00\x00\x00\x00\x00\x00\x00'

class Recorder:
    def __init__(self): self.names = []
    def refused(self, name, via): self.names.append((name, via))

allow = ({'github.com'}, set())
upstream = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
upstream.bind(('127.0.0.1', 0))
def resolver():
    request, client = upstream.recvfrom(65535)
    upstream.sendto(request[:2] + b'answer', client)
threading.Thread(target=resolver).start()
log = Recorder()
assert answer(query(b'github.com'), allow, log, upstream.getsockname()) == b'\x12\x34answer'
assert log.names == []

refused = answer(query(b'example.com', b'\xab\xcd', 16), allow, log, ('127.0.0.1', 9))
assert log.names == [('example.com', 'dns')]
assert refused[:2] == b'\xab\xcd'
flags = int.from_bytes(refused[2:4], 'big')
assert flags & 0x8000 and flags & 0x0100 and flags & 0x0080 and flags & 0xf == 3, hex(flags)
assert refused[4:12] == b'\x00\x01\x00\x00\x00\x00\x00\x00'
assert refused[12:] == query(b'example.com', qtype=16)[12:12 + 13 + 4]

# A reply, two questions and a compressed name are not queries that the resolver answers.
assert answer(b'\x00\x00\x81\x00' + query(b'github.com')[4:], allow, log) is None
assert answer(query(b'github.com')[:4] + b'\x00\x02' + query(b'github.com')[6:], allow, log) is None
assert answer(query(b'github.com')[:12] + b'\xc0\x0c\x00\x01\x00\x01', allow, log) is None
assert answer(b'\x00' * 11, allow, log) is None
",
    );
}

#[test]
#[cfg(unix)]
fn refused_names_are_logged_once_a_minute_and_only_into_a_private_folder() {
    python(
        r"
os.mkdir('log', 0o755)
clock = [1000]
log = Log('log', now=lambda: clock[0])
for _ in range(3):
    log.refused('Danger.example.', 'tls')
log.refused('danger.example', 'dns')
log.refused('not a name', 'tls')
lines = [json.loads(line) for line in open('log/refused.jsonl')]
assert lines == [{'at': 1000, 'name': 'danger.example', 'via': 'tls'}, {'at': 1000, 'name': '-', 'via': 'tls'}], lines
assert stat.S_IMODE(os.stat('log/refused.jsonl').st_mode) == 0o644
clock[0] += 60
log.refused('danger.example', 'dns')
assert len(open('log/refused.jsonl').readlines()) == 3
for n in range(40):
    log.refused('n%d.example' % n, 'dns')
assert len(open('log/refused.jsonl').readlines()) == 3 + 29

# A link in place of the log, or a folder that others can write to, gets nothing.
os.remove('log/refused.jsonl')
os.symlink('/dev/null', 'log/refused.jsonl')
Log('log', now=lambda: 0).refused('a.example', 'tls')
os.remove('log/refused.jsonl')
os.chmod('log', 0o775)
Log('log', now=lambda: 0).refused('a.example', 'tls')
assert not os.path.exists('log/refused.jsonl')
os.chmod('log', 0o755)
os.mkfifo('log/refused.jsonl')
Log('log', now=lambda: 0).refused('a.example', 'tls')
os.remove('log/refused.jsonl')
os.symlink('log', 'linked')
Log('linked', now=lambda: 0).refused('a.example', 'tls')
assert not os.path.exists('log/refused.jsonl')

# A full log starts again.
with open('log/refused.jsonl', 'wb') as full:
    full.write(b'x' * (LOG_LIMIT + 1))
Log('log', now=lambda: 0).refused('a.example', 'tls')
assert open('log/refused.jsonl').read() == json.dumps({'at': 0, 'name': 'a.example', 'via': 'tls'}) + '\n'
",
    );
}

#[test]
fn only_a_hetzner_profile_takes_an_allowlist_and_one_without_it_is_saved_as_before() {
    let mut profile = crate::CloudConfig::parse(crate::EXAMPLE)
        .unwrap()
        .profiles
        .remove("image-only")
        .unwrap();
    assert!(profile.egress.is_none());
    assert!(serde_json::to_value(&profile).unwrap().get("egress").is_none());
    profile.provider = crate::hetzner::PROVIDER.into();
    profile.egress = Some(Egress::default());
    assert!(profile.validate(false).is_ok());
    let saved = serde_json::to_value(&profile).unwrap();
    assert_eq!(saved["egress"], serde_json::json!({}));
    assert_eq!(serde_json::from_value::<crate::Profile>(saved).unwrap(), profile);
    profile.egress = Some(Egress {
        allow: ["*.com".to_owned()].into(),
    });
    assert!(profile.validate(false).is_err());
    profile.egress = Some(Egress::default());
    profile.provider = "runpod".into();
    assert!(profile.validate(false).is_err());
}
