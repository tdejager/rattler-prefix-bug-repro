//! Build victim with a long placeholder, rewrite it through rattler with a
//! shorter target, and run it. Exit codes:
//!   0 — bug reproduced (rewritten victim exits non-zero)
//!   1 — bug not reproduced (rewritten victim exits zero)
//!   2 — harness itself failed

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const PLACEHOLDER: &str = "/PLACEHOLDPLACEHOLD/PLACEHOLDPLACEHOLD/PLACEHOLDPLACEHOLD/PLACEHOLDPLACEHOLD/lib/test/runtimedep";
const TARGET: &str = "/short";

fn main() -> ExitCode {
    match run() {
        Ok(true) => {
            println!("\n*** bug reproduced ***");
            ExitCode::from(0)
        }
        Ok(false) => {
            println!("\n*** bug NOT reproduced — rewritten victim ran clean ***");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("harness error: {e}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<bool, String> {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("no parent of CARGO_MANIFEST_DIR")?
        .to_path_buf();
    let victim = workspace.join("target/release/victim");
    let rewritten = workspace.join("target/release/victim-rewritten");

    println!("=== build victim with BAKED={PLACEHOLDER:?} ===");
    cmd(Command::new("cargo")
        .args(["build", "--release", "-p", "victim"])
        .env("BAKED", PLACEHOLDER)
        .current_dir(&workspace))?;

    println!("\n=== run unmodified victim ===");
    if !cmd(&mut Command::new(&victim))?.success() {
        return Err("unmodified victim already exits non-zero".into());
    }

    println!("\n=== rewrite victim with rattler::install::link::copy_and_replace_cstring_placeholder ===");
    rewrite(&victim, &rewritten)?;
    #[cfg(target_os = "macos")]
    cmd(Command::new("codesign").args(["-s", "-", "-f"]).arg(&rewritten))?;

    println!("\n=== run rewritten victim ===");
    let status = cmd(&mut Command::new(&rewritten))?;
    let code = status.code().ok_or(
        "rewritten victim killed by signal — likely a bad code signature, \
         not the bug we are reproducing",
    )?;
    println!("(exit code {code})");
    Ok(code != 0)
}

fn rewrite(src: &Path, dst: &Path) -> Result<(), String> {
    let bytes = std::fs::read(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    let mut out = Vec::with_capacity(bytes.len());
    rattler::install::link::copy_and_replace_cstring_placeholder(&bytes, &mut out, PLACEHOLDER, TARGET)
        .map_err(|e| format!("rattler rewrite: {e}"))?;
    std::fs::write(dst, &out).map_err(|e| format!("write {}: {e}", dst.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(src).map_err(|e| format!("{e}"))?.permissions().mode();
        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode))
            .map_err(|e| format!("{e}"))?;
    }
    Ok(())
}

fn cmd(c: &mut Command) -> Result<std::process::ExitStatus, String> {
    c.status().map_err(|e| format!("{c:?}: {e}"))
}
