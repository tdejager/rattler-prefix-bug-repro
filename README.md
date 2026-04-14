# rattler prefix relocation bug — minimal reproducer

Demonstrates that `rattler::install::link::copy_and_replace_cstring_placeholder`
silently corrupts baked `&'static str` constants in installed Rust binaries..

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

**Linker suffix merging (`DEP`, `BUILD`).** This is a documented linker
optimisation: identical and tail-overlapping read-only string literals are
deduplicated, with shorter literals' references rewritten to point into the
*middle* of a longer literal that ends with the same bytes. On ELF the
optimisation is gated by the `SHF_MERGE | SHF_STRINGS` section flags
documented in the [ELF gABI section header
spec](https://refspecs.linuxfoundation.org/elf/gabi4+/ch4.sheader.html#sh_flags);
on Mach-O it's the analogous `S_CSTRING_LITERALS` section type. The
implementation in LLVM lives in [`llvm::StringTableBuilder`](https://llvm.org/doxygen/classllvm_1_1StringTableBuilder.html),
which documents that it "deduplicates identical strings, and also performs
suffix merging".

In this reproducer, with LTO and `codegen-units = 1`, `DEP` and `BUILD` get
suffix-merged into the tail of `BAKED`. After the rewriter's compact-shift,
the fixed offsets where `DEP` and `BUILD` used to alias now land inside the
trailing NUL padding, so the constants read `\0\0\0` and `\0\0\0\0\0` even
though their original bytes still exist elsewhere in the binary under
different pointers.

This failure mode is the worse of the two: it's silent (no `CString` error
to catch), it depends on rustc/LLVM/linker version and optimisation flags,
and it can affect any short read-only string constant in any tool that
also bakes a path containing the constant's bytes as a suffix. The bytes
that get clobbered look like junk const data; the bug surfaces only when
some hot-path code actually dereferences the affected constant.

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
