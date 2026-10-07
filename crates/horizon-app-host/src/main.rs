#![forbid(unsafe_code)]
fn main() {
    horizon_app_host::entry::execute(&std::env::args().skip(1).collect::<Vec<_>>());
}
