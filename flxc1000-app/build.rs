use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=FLXC1000_VERSION");

    // Watch the Git metadata that affects the displayed version.
    for path in ["HEAD", "index", "packed-refs"] {
        watch_git_path(path);
    }

    // Watch the current branch reference, if HEAD points to a branch.
    if let Some(branch) = git_output(&["symbolic-ref", "--quiet", "HEAD"]) {
        watch_git_path(&branch);
    }

    // Source changes can affect the --dirty suffix.
    println!("cargo:rerun-if-changed=src");

    let version = std::env::var("FLXC1000_VERSION")
        .ok()
        .or_else(git_version)
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".into()));

    println!("cargo:rustc-env=FLXC1000_BUILD_VERSION={version}");
}

fn watch_git_path(path: &str) {
    if let Some(path) = git_output(&["rev-parse", "--git-path", path]) {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;

    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_version() -> Option<String> {
    git_output(&["describe", "--tags", "--always", "--dirty"])
}
