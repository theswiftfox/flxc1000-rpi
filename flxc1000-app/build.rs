use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=FLXC1000_VERSION");
    println!("cargo:rerun-if-changed=../.git/HEAD");

    let version = std::env::var("FLXC1000_VERSION")
        .ok()
        .or_else(git_version)
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".into()));

    println!("cargo:rustc-env=FLXC1000_BUILD_VERSION={version}");
}

fn git_version() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
