include!("../shared/fw_version_build.rs");

fn main() {
    emit_fw_version();
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}
