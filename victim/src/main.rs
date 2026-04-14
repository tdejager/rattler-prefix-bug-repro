const BAKED: &str = env!("BAKED");
const DEP: &str = "dep";
const BUILD: &str = "build";

fn main() {
    // black_box defeats the constant-folding that LTO+codegen-units=1 would
    // otherwise apply to comparisons against these consts, which would
    // collapse the corruption check into compile-time `false` and miss
    // the runtime corruption entirely.
    let baked = std::hint::black_box(BAKED).as_bytes();
    let dep = std::hint::black_box(DEP).as_bytes();
    let build = std::hint::black_box(BUILD).as_bytes();

    println!("BAKED.len            = {}", baked.len());
    println!("BAKED.contains_nul   = {}", baked.contains(&0));
    println!("BAKED (first 40 B)   = {}", escape(baked, 40));
    println!("DEP                  = {} (bytes: {dep:?})", escape(dep, 32));
    println!("BUILD                = {} (bytes: {build:?})", escape(build, 32));
    println!("DEP   == b\"dep\"      = {}", dep == b"dep");
    println!("BUILD == b\"build\"    = {}", build == b"build");
    match std::ffi::CString::new(baked) {
        Ok(_) => println!("CString::new(BAKED)  = ok"),
        Err(e) => println!("CString::new(BAKED)  = error ({e})"),
    }

    // Exit non-zero if anything changed from compile time. We don't print a
    // textual verdict here — the format string would itself be subject to
    // any byte-shift the rewriter does, leading to confusing nested
    // corruption in the output. The harness reads the exit code.
    if baked.contains(&0) || dep != b"dep" || build != b"build" {
        std::process::exit(1);
    }
}

fn escape(s: &[u8], max: usize) -> String {
    let mut out = String::from("\"");
    for &b in s.iter().take(max) {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('"');
    if s.len() > max {
        out.push('…');
    }
    out
}
