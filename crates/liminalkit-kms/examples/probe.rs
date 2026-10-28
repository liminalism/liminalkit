//! What displays this machine has, read-only: no mode is set, so it is safe
//! to run under a desktop. `cargo run -p liminalkit-kms --example probe`
fn main() {
    for line in liminalkit_kms::probe() {
        println!("{line}");
    }
}
