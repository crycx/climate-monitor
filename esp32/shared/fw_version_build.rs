// Build-script helper, `include!`d by the firmware crates' build.rs.
//
// Sets `FW_VERSION` to the git commit (7 hex digits), with "-dirty" when
// tracked files have local changes, or "unknown" without git. The firmware
// reports it to Home Assistant and puts it in the app descriptor, so an OTA
// image can be told apart from the one running.

fn emit_fw_version() {
    use std::process::Command;

    let git = |args: &[&str]| -> Option<String> {
        // --no-optional-locks: do not rewrite .git/index (that would rerun this script).
        let out = Command::new("git").arg("--no-optional-locks").args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };

    let version = match git(&["rev-parse", "--short=7", "HEAD"]) {
        Some(hash) => {
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
            if dirty { format!("{}-dirty", hash) } else { hash }
        }
        None => "unknown".into(),
    };
    println!("cargo:rustc-env=FW_VERSION={}", version);

    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={}/HEAD", dir);
        println!("cargo:rerun-if-changed={}/index", dir);
        if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={}/{}", dir, head_ref);
        }
    }
    // Editing sources changes "-dirty" before anything is staged.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../shared");
    println!("cargo:rerun-if-changed=build.rs");
}
