fn main() {
    // Prefer a hash supplied by the build environment. Nix builds run in a
    // sandbox with no `.git`, so the flake passes its own git rev in as
    // GIT_HASH; this is what gives container images a real commit hash. For
    // local/dev cargo builds GIT_HASH is unset, so we fall back to invoking
    // git, and finally to "unknown".
    println!("cargo:rerun-if-env-changed=GIT_HASH");

    let hash = std::env::var("GIT_HASH")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_HASH={hash}");
    println!("cargo:rerun-if-changed=../.git/HEAD");
}
