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

```
key:  [ ... ]   bytes in .rodata
      ^════     a Rust &'static str — fixed pointer + compile-time length
      .         NUL byte


BEFORE rewrite — build-time layout in .rodata:

   [ P L A C E H O L D E R / r u n t i m e d e p . ]
     ^═════════════════ BAKED ═════════════════════
                                          ^═══ DEP   (linker suffix-merged
                                                      into BAKED's tail)


AFTER rewrite — rattler replaces `PLACEHOLDER` with `/short` and pads the
freed bytes at the end of the C-string region with NULs:

   [ / s h o r t / r u n t i m e d e p . . . . . . ]
     ^═════════════════ BAKED ═════════════════════
                                          ^═══ DEP

   BAKED reads as:  "/short/runtimedep\0\0\0\0\0\0"
                                       ^^^^^^^^^^^^
                                       embedded NULs are inside the
                                       slice → first CString::new fails

   DEP reads as:    "\0\0\0"
                    ^^^^^^^^
                    DEP's compile-time offset now lies in the trailing
                    NUL padding; the original "dep" bytes still exist
                    elsewhere in the binary, just not under this pointer
```

The `(ptr, len)` slice header for both `BAKED` and `DEP` is finalised at
compile time and rattler never updates it. Both failure modes follow from
that, and they reproduce together in this minimal example.

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


### Caveats

- The aliasing failure depends on rustc/LLVM/linker versions and
  optimisation flags. The workspace pins `lto = "fat"` and
  `codegen-units = 1` to make it more likely. Tested on macOS arm64 with
  the Rust toolchain pinned in `pixi.toml`; if you can't reproduce on
  another target that's itself a useful data point.
- On macOS the harness re-signs the rewritten binary with an ad-hoc
  signature (`codesign -s -`) before running it, because modifying a
  Mach-O invalidates its signature and the kernel will SIGKILL it at
  exec. rattler's real install path does the equivalent.
