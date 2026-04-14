# rattler prefix relocation bug — minimal reproducer

Demonstrates that `rattler::install::link::copy_and_replace_cstring_placeholder`
silently corrupts baked `&'static str` constants in installed Rust binaries.
No `rattler-build`, no recipe, no conda metadata — one `pixi` task builds a
tiny victim binary, calls the rattler API directly, and observes the
corruption.

## Reproduce

```sh
pixi run repro
```

Expected tail:

```
=== run rewritten victim ===
BAKED.len            = 96
BAKED.contains_nul   = true
BAKED (first 40 B)   = "/shortdepbuildCString::new(BAKED)  = ok"…
DEP                  = "" (bytes: [0, 0, 0])
BUILD                = "" (bytes: [0, 0, 0, 0, 0])
DEP   == b"dep"      = false
BUILD == b"build"    = false
CString::new(BAKED)  = error (nul byte found in provided data at position: 45)
(exit code 1)

*** bug reproduced ***
```

Harness exit codes: `0` reproduced, `1` not reproduced, `2` harness error.

## What the rewriter does

`copy_and_replace_cstring_placeholder` walks forward from each placeholder
occurrence to the next `\0`, replaces every placeholder match inside that
NUL-bounded region with the (shorter) target prefix, compacts the surviving
bytes adjacent to the new prefix, and pads the freed bytes at the **end** of
the region with `\0`. The total region length is preserved; byte offsets
inside it are not, and Rust slice metadata is never updated.

## Two failure modes, both visible above

**Trailing-pad (`BAKED`).** The baked `&str`'s `(ptr, len)` was finalised at
compile time. After the rewrite, reading `len` bytes from `ptr` yields
`<actual_prefix><tail><NUL padding>`. `PathBuf::from` and string formatting
accept the embedded NULs silently, but `CString::new` rejects them — so the
first syscall (`open`, `stat`, `realpath`, …) fails. This is what surfaces in
the reproducer as `CString::new(BAKED) = error (nul byte found at position 45)`.

**Linker-merge aliasing (`DEP`, `BUILD`).** Linkers (LLD, ld64) deduplicate
string literals by tail-merging: if `"dep"` appears as a suffix of any longer
literal, the linker can free the `"dep"` allocation and rewrite all
references to point into the middle of the longer string. With LTO and
codegen-units=1, this happens here for `DEP` and `BUILD`, which get aliased
into the tail of `BAKED`. After the compact-shift, those fixed offsets land
inside the trailing NUL padding, so the constants now read `\0\0\0` and
`\0\0\0\0\0` — even though their original bytes still exist elsewhere in
the binary under different pointers.

The aliasing failure is the worse of the two: it's silent (no `CString`
error to catch), version-dependent (different rustc/linker combos give
different results), and can affect any const literal short enough to be
suffix-merged into a baked path. It's the failure mode that broke
[inko](https://inko-lang.org) on conda-forge — `pub const DEP: &str = "dep"`
became `\0\0\0`, `cwd.join(DEP)` produced `cwd/\0\0\0`, and `inko check
hello.inko` failed at every invocation. See [conda-forge/staged-recipes#32967](https://github.com/conda-forge/staged-recipes/pull/32967).

## What we'd like rattler to consider

- **Document the failure mode** in the Rust packaging guidance and recommend
  exe-relative discovery (via `current_exe()` / `argv[0]` / `dladdr`) over
  baked `env!()` paths. `rustc`, `zig`, `clang`, `cpython`, and Ruby with
  `LOAD_RELATIVE` already do this.
- **Optional:** emit a packaging-time warning when a placeholder occurrence
  sits inside a NUL-bounded region that contains additional content past
  the obvious path tail — that's the structural signature of merged
  literals being adjacent to a baked path.
- **Optional:** reconsider the in-region replacement strategy. The current
  "compact + trailing pad" corrupts aliased neighbours but leaves the baked
  path's bytes intact. Writing `<actual_prefix><NUL padding><original tail>`
  instead would corrupt the baked path itself with mid-string NULs (breaks
  at the first syscall, loud and immediate) but leaves aliased neighbours
  alone. Different trade-off; both seem better than silent collateral
  damage in some scenarios.

What rattler can't realistically do: walk Rust slice metadata to update
`(ptr, len)` pairs after a shift, or prevent the linker from merging
strings in the first place.

## Layout

```
.
├── pixi.toml          rust + cargo from conda-forge
├── Cargo.toml         workspace; release profile uses LTO=fat, codegen-units=1
├── victim/src/main.rs ~50 lines — baked &str + adjacent short consts
└── harness/src/main.rs ~80 lines — builds victim, calls rattler, runs
```

## Caveats

- The aliasing failure depends on rustc/LLVM/linker versions and
  optimisation flags. The workspace pins `lto = "fat"` and
  `codegen-units = 1` to make it more likely. Tested on macOS arm64 with
  the Rust toolchain pinned in `pixi.toml`; if you can't reproduce on
  another target that's itself a useful data point.
- On macOS the harness re-signs the rewritten binary with an ad-hoc
  signature (`codesign -s -`) before running it, because modifying a
  Mach-O invalidates its signature and the kernel will SIGKILL it at
  exec. rattler's real install path does the equivalent.
