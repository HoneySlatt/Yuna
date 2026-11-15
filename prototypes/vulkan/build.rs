//! Compiles the shaders to SPIR-V with glslangValidator, and names the commit the prototype is
//! built from, which the report prints beside its figures (ADR-0073).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The shaders in `shaders/`, each compiled to `$OUT_DIR/<name>.spv`.
const SHADERS: [&str; 5] = [
    "pattern.vert",
    "pattern.frag",
    "sample.comp",
    "window.vert",
    "window.frag",
];

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    for shader in SHADERS {
        compile(shader, &out);
    }
    println!("cargo::rustc-env=PROTOTYPE_COMMIT={}", commit());
    // On NixOS no libvulkan.so.1 is on the library path: the Nix shell and package name the
    // loader's folder, which goes on the run path that dlopen reads.
    println!("cargo::rerun-if-env-changed=VULKAN_LOADER_DIR");
    if let Ok(folder) = env::var("VULKAN_LOADER_DIR") {
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,{folder}");
    }
}

fn compile(shader: &str, out: &Path) {
    let source = format!("shaders/{shader}");
    println!("cargo::rerun-if-changed={source}");
    let status = Command::new("glslangValidator")
        .args(["-V", "--target-env", "vulkan1.3", "-o"])
        .arg(out.join(format!("{shader}.spv")))
        .arg(&source)
        .status();
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => panic!("glslangValidator failed on {source}: {status}"),
        Err(error) => {
            panic!("glslangValidator could not run ({error}): build inside `nix develop`")
        }
    }
}

/// The commit: given by the Nix package, which has no `.git`, or read from git, with `-dirty`
/// when the prototype's own files differ from it.
fn commit() -> String {
    println!("cargo::rerun-if-env-changed=PROTOTYPE_COMMIT");
    if let Ok(commit) = env::var("PROTOTYPE_COMMIT") {
        return commit;
    }
    // A commit or a checkout writes these, and editing the sources changes the dirty state.
    for path in ["HEAD", "index", "logs/HEAD"] {
        if let Some(path) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo::rerun-if-changed={path}");
        }
    }
    println!("cargo::rerun-if-changed=src");
    println!("cargo::rerun-if-changed=shaders");
    let Some(head) = git(&["rev-parse", "HEAD"]) else {
        return "unknown".to_owned();
    };
    match git(&[
        "status",
        "--porcelain",
        "--untracked-files=normal",
        "--",
        ".",
    ]) {
        Some(changes) if changes.is_empty() => head,
        _ => format!("{head}-dirty"),
    }
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
