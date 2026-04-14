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

### Failure mode 1 — trailing pad on the baked path itself

The baked `&str`'s `(ptr, len)` is finalised at compile time. After the
rewrite the byte length is preserved but the new prefix is shorter, so the
freed bytes show up as NULs inside the slice.

```
key:  [ ... ]   bytes in .rodata
      ^════     a Rust &'static str — fixed pointer + compile-time length
      .         NUL byte (C-string terminator)


BEFORE rewrite:

   [ P L A C E H O L D E R / f o o . ]
     ^══════════ BAKED ═════════════


AFTER rewrite (placeholder → /short):

   [ / s h o r t / f o o . . . . . . ]
     ^══════════ BAKED ═════════════

   BAKED reads as:  "/short/foo\0\0\0\0\0\0"
                              ^^^^^^^^^^^^
                              embedded NULs inside the slice
                              → first CString::new(BAKED) fails
```

This mode requires no linker optimisation. It will fire for any baked path
that anything reads into a CString-equivalent (`open`, `stat`, `realpath`,
…). A tool that only ever uses the baked path for `Display` / logging
hits the bug invisibly: the pad bytes are there, nothing notices.

### Failure mode 2 — suffix-merged const aliased into the baked path

The linker (LLD, ld64) deduplicates string literals by suffix merging: if
`"dep"` is a suffix of any longer literal in the same section, the linker
can free the standalone `"dep"` allocation and rewrite all references to
point into the *middle* of the longer literal. When that longer literal
happens to be the baked path, the aliased const's compile-time pointer
ends up inside a region that rattler is going to shift.

```
BEFORE rewrite:

   [ P L A C E H O L D E R / d e p . ]
     ^══════════ BAKED ═════════════
                            ^═══ DEP   (aliased into BAKED's tail)


AFTER rewrite (placeholder → /short):

   [ / s h o r t / d e p . . . . . . ]
     ^══════════ BAKED ═════════════
                            ^═══ DEP

   DEP reads as:    "\0\0\0"
                    ^^^^^^^^
                    DEP's compile-time offset now lies in the trailing
                    NUL padding. The "dep" bytes still exist elsewhere
                    in the binary, just not under this pointer.
```

This mode is independent of whether the program ever uses `BAKED`. A tool
that bakes a path purely for logging can still ship broken consts because
the linker aliased them in. The `(ptr, len)` slice header for both `BAKED`
and `DEP` is finalised at compile time and rattler never updates it; both
modes follow from that.

The two modes can occur together (as in this reproducer, where both fire
from the same rewrite) or in isolation — a binary with no suffix-merged
consts hits only mode 1 if it syscalls the baked path, and a binary that
never syscalls its baked path hits only mode 2 if any short const got
aliased.

## Linker references for mode 2

Suffix merging is a documented linker optimisation, not an LTO quirk. On
ELF it is gated by the `SHF_MERGE | SHF_STRINGS` section flags described in
the [ELF gABI section header spec](https://refspecs.linuxfoundation.org/elf/gabi4+/ch4.sheader.html#sh_flags);
on Mach-O it is the analogous `S_CSTRING_LITERALS` section type. The
implementation in LLVM lives in [`llvm::StringTableBuilder`](https://llvm.org/doxygen/classllvm_1_1StringTableBuilder.html),
which documents that it "deduplicates identical strings, and also performs
suffix merging". This reproducer pins `lto = "fat"` and `codegen-units = 1`
in `Cargo.toml` to make merging more likely, but neither is required for
the underlying optimisation to fire.


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
