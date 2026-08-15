//! WINARM (P5): the Windows API knowledge-base resolver (`docs/FFI.md`,
//! MIGRATION.md §3.5, `docs/sprints/sprint_p05_detail.md`).
//!
//! Taken from WINVM's `src/runtime/winkb.rs` — a Windows-only file, so
//! wholesale is correct (MIGRATION.md §2's source-of-truth rule) — with its
//! ABI **classifier section replaced**: WINVM's encoded **Win-x64** rules
//! (struct > 8 B ⇒ "hidden pointer" ⇒ refuse), and this target is **MS
//! ARM64**, whose composite rules differ (§D2). The re-derivation is this
//! sprint's substance, and every cell of it is either modelled WITH an
//! execution pin or refused WITH a named reason — never guessed.
//!
//! MACVM's FFI is data-driven — it queries `cocoa_data/cocoa.sqlite` and
//! "never re-derives from the SDK". This is the Windows substitution of
//! that data source, not a new design: `windows_api.db`, the SQLite
//! knowledge base built from Win32Metadata (70.0.11-preview here; row
//! counts on this machine: 18,271 functions / 60,780 params / 46,250 COM
//! methods with vtable indices / 97,402 constants / 66,708 struct fields).
//!
//! ## The MS-ARM64 classifier (D2), as built
//!
//! | case | AAPCS64/MS rule | v1 |
//! |---|---|---|
//! | int/pointer/handle/enum scalar ≤ 8 B | one GPR | **model** (`G`) |
//! | `f64` | one d-register | **model** (`F`) |
//! | `f32` | the **s**-register (low 32 bits of the v-reg) | **refuse** — the marshal shape is `f64::to_bits` into a d-reg (`runtime::ffi::marshal_f`); an f32 through it passes garbage bits, silently |
//! | struct ≤ 8 B, provably FP-free fields | one GPR | **model** (`G`) — HANDLE, HWND, BOOL, PSTR, POINT, FILETIME |
//! | struct ≤ 16 B with any FP field | HFA ⇒ v0–v3 (s/d regs), mixed ⇒ GPRs — which one requires layout analysis | **refuse**, naming the HFA hazard (`D2D_POINT_2F` is 8 B of two floats: as `G` its bits would land in an x-reg while the callee reads s0/s1) |
//! | struct 9–16 B, FP-free | TWO GPRs (x0:x1 for returns) | **refuse**: one pragma token maps to one 64-bit slot, so a two-register composite is not expressible end to end; the ABI fact itself is execution-pinned by `it_codecache::struct16_return_in_x0_x1` |
//! | struct > 16 B non-HFA | indirect (returns via the x8 pointer) | **refuse**, naming x8 — the exact bug class WINVM's hidden-pointer finding documents ("a pointer plus garbage with nothing failing at the point of error") |
//! | **variadic** | ALL variadic args in GPRs, floats bit-copied to x-regs; Apple passes them on the STACK | **refuse** — the trampolines were derived for the fixed-arity convention (floats in d-regs, int args 9+ spilled) and would place variadic floats wrongly with no fault |
//! | callback / function pointer (`delegate` kind) | a code address in a GPR | **model** (`G`) — see `docs/win_gui_design.md` §2.2: WG passes Rust trampoline addresses (`RegisterClassW`'s wndproc, `SetTimer`'s TIMERPROC); there is no Smalltalk-side callback marshalling, so the pointer is just a number, and the refusal stance must not catch a case that IS modelled trivially |
//! | COM method | vtable index from the DB, `this` in x0 | **model** the two facts ([`lookup_com_method`]); the dispatch arm itself is the future `win_gui` track's, not this sprint's |
//!
//! ## Δ (P5, measured): `is_variadic` is all-zero in this DB build
//!
//! `SELECT COUNT(*) FROM functions WHERE is_variadic=1` → **0** (of
//! 18,271); `wsprintfA` — genuinely variadic — carries `is_variadic = 0`
//! and only its two fixed params. The winmd this DB was built from does
//! not record the vararg tail, so the variadic refusal above cannot fire
//! from data in practice. It is kept as a belt for future DB builds; the
//! **live guard is the strict arity/mask cross-check in
//! `runtime::ffi::resolve_ffi_symbol`**: a call site passing tail
//! arguments to a DB-known function disagrees with the recorded fixed
//! arity and refuses loudly, which closes the reachable half of the hole.
//! (A pragma matching the fixed arity exactly and a format string that
//! reads no tail is indistinguishable from a fixed call at every level —
//! including the callee's.)
//!
//! ## `extern "system"` ≡ `extern "C"` on ARM64
//!
//! There is no stdcall legacy on this target; the DB's `callconv` column
//! ("winapi"/"cdecl", both AAPCS64 here) is asserted cheaply on read
//! anyway, so an x86-flavoured row leaking through a future query refuses
//! instead of dispatching under the wrong assumption.
//!
//! ## `GetLastError` discipline (sprint doc pitfalls)
//!
//! A failing Win32 call's error code is per-thread state that the next
//! Win32 call — from ANY layer — clobbers. v1 deliberately does **no
//! implicit capture** in the trampolines or the dispatch path: guest code
//! that wants the error binds `GetLastError` as its own zero-arg import
//! and calls it as the IMMEDIATELY next FFI call **through an
//! already-warmed binding**. The warming clause is measured, not
//! theoretical: a binding's FIRST call runs resolution, and resolution
//! itself performs Win32 calls (this module's sqlite open,
//! `GetModuleHandleA`/`LoadLibraryA`/`GetProcAddress`), which reset the
//! thread's last-error — so `fail(); GetLastError()` reads 0 if the
//! `GetLastError` binding was never called before. Once both bindings are
//! warm the sequence works, because nothing between two CACHED guest FFI
//! sends performs a Win32 call (marshalling and SMI/Double unmarshalling
//! are syscall-free). A discipline, not a guarantee — documented here and
//! at the world binding (`world/tests/59_win_io_tests.mst`), pinned by
//! `embed::tests::win32_get_last_error_reads_the_failing_calls_code`.
//!
//! ## What this module refuses, and why that matters
//!
//! A resolver that guesses is worse than no resolver: a mis-classified
//! parameter does not fault, it silently passes the wrong bits in the
//! wrong register file. So anything that cannot be modelled exactly is an
//! explicit [`WinkbError::Unsupported`] naming the rule it fell to,
//! rather than a best effort.
//!
//! ## Absence is not an error
//!
//! The database is a ~90 MB machine-local artifact, not a build input.
//! When it is missing every lookup returns [`WinkbError::DbMissing`] and
//! the caller falls back to the hand-declared types in the
//! `<primitive: FFI ...>` pragma plus the [`resolve_export`] probe —
//! exactly the no-DB CI story. Nothing about the build or the test suite
//! depends on the file existing (tests_p05.md gate item 5 runs the whole
//! suite both ways). To (re)build the artifact, RASM's `winkb` crate is
//! the generator; this machine's copy came from the WRASM release
//! `v1.0-db` (Win32Metadata 70.0.11-preview).
//!
//! ## Why the LoadLibrary/GetProcAddress helper lives HERE
//!
//! P1's Δ moved `dlsym_resolve`'s Windows twin out of the vendored loader
//! (`vendor/wfasm/native_winarm64.rs` deliberately carries only the JIT
//! memory surface) and into this sprint. Its consumers are all
//! runtime-side FFI — `runtime/ffi.rs` today; the objc twins are
//! mac-gated stubs on this target — so the resolver module that already
//! owns "how a name becomes an address" is the natural home:
//! [`resolve_export`], with the three kernel32 imports hand-declared
//! locally (`extern "system"`, the P0 convention), and the vendor seam
//! untouched.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Which register file an argument or return value travels in — the only
/// classification the marshaller needs, since every integer, pointer and
/// handle is passed the same way on AAPCS64.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArgClass {
    /// Integer, pointer, handle, enum, FP-free struct ≤ 8 B — a general
    /// register (x0–x7, then the trampoline's stack-spill slots).
    G,
    /// `f64` — a d-register. **Not `f32`** on this target: the marshal
    /// shape is `f64::to_bits` into the full d-reg, while an `f32`
    /// travels in the s-register (the low 32 bits), so classifying f32
    /// here would pass garbage without faulting — it refuses instead.
    F,
    /// `void`. Valid only as a return class.
    V,
}

/// A resolved native function signature.
#[derive(Clone, Debug)]
pub struct FnSig {
    /// The exported name, as it must be passed to `GetProcAddress`
    /// (measured: `export_name` never differs from `function_name` in
    /// this DB build).
    pub name: String,
    /// The DLL that exports it, e.g. `KERNEL32.dll` (never NULL in this
    /// build — the guard stays as a belt).
    pub dll: String,
    pub ret: ArgClass,
    /// Argument classes in signature position order — the order AAPCS64
    /// assigns slots in, and the order `codecache::ffi_stubs`' buffers
    /// expect.
    pub params: Vec<ArgClass>,
    /// See the module doc's Δ: all-zero in this DB build; kept as a belt
    /// for future builds (a `1` refuses at lookup).
    pub is_variadic: bool,
}

impl FnSig {
    /// Bit `i` set means argument `i` travels in the FP register file —
    /// the cross-check mask `runtime::ffi` compares against the pragma's
    /// own token list before the first call.
    pub fn class_mask(&self) -> u32 {
        let mut m = 0u32;
        for (i, c) in self.params.iter().enumerate() {
            if *c == ArgClass::F {
                m |= 1 << i;
            }
        }
        m
    }
}

/// One COM interface method, enough to dispatch it: the vtable slot and
/// the argument classes (`this` included at position 0). The dispatch arm
/// that would consume this is the future `win_gui` track's (sprint doc
/// "Interfaces for later sprints"); P5 models and pins the DATA.
#[derive(Clone, Debug)]
pub struct ComMethod {
    pub interface: String,
    /// The interface's IID, as the canonical hyphenated GUID string.
    pub iid: String,
    pub method: String,
    /// Index into the object's vtable. Inherited methods are counted, so
    /// a direct `IUnknown` child's own methods start at 3.
    pub vtable_index: i64,
    /// Includes the implicit `this` pointer at position 0, because that
    /// is how the call is actually made.
    pub params: Vec<ArgClass>,
    pub ret: ArgClass,
}

#[derive(Debug)]
pub enum WinkbError {
    /// No database on this machine — the caller should fall back to the
    /// hand-declared pragma types plus the [`resolve_export`] probe.
    DbMissing(PathBuf),
    /// The database is present but this symbol is not in it.
    NotFound(String),
    /// Present, but its signature cannot be modelled exactly — the
    /// message names the classifier rule it fell to.
    Unsupported(String),
    Db(String),
}

impl std::fmt::Display for WinkbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WinkbError::DbMissing(p) => write!(
                f,
                "windows_api.db not found at {} (set WINKB_DB to override)",
                p.display()
            ),
            WinkbError::NotFound(n) => write!(f, "{n} is not in windows_api.db"),
            WinkbError::Unsupported(m) => write!(f, "{m}"),
            WinkbError::Db(m) => write!(f, "windows_api.db: {m}"),
        }
    }
}

/// Where the database lives: `WINKB_DB` if set, else this machine's copy
/// (delivered from the WRASM `v1.0-db` release). WINVM's default pointed
/// at ITS machine's drive (`E:\...`); the override env var is the shared
/// convention and is kept verbatim.
pub fn db_path() -> PathBuf {
    match std::env::var("WINKB_DB") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => PathBuf::from(r"C:\projects\windows_api\windows_api.db"),
    }
}

/// Whether a knowledge base is available on this machine. Cached: the
/// answer cannot change during a run, and callers ask per FFI resolution.
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| db_path().is_file())
}

#[cfg(windows)]
fn open() -> Result<rusqlite::Connection, WinkbError> {
    open_at(&db_path())
}

/// Split from [`open`] so the fixture-DB tests (the "DB is data, not
/// gospel" boundary — tests_p05.md's deliberately-wrong-row stress) can
/// aim the whole lookup path at a synthetic file without touching the
/// process-global `WINKB_DB` (env mutation races parallel tests).
#[cfg(windows)]
fn open_at(p: &Path) -> Result<rusqlite::Connection, WinkbError> {
    if !p.is_file() {
        return Err(WinkbError::DbMissing(p.to_path_buf()));
    }
    // Read-only: this is a shared, machine-wide artifact and nothing here
    // has any business writing to it.
    rusqlite::Connection::open_with_flags(
        p,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| WinkbError::Db(e.to_string()))
}

// ── The MS-ARM64 classifier (D2 — the replaced section) ─────────────────────

/// What a struct's recorded fields say about its register class.
#[cfg(windows)]
enum FieldContent {
    /// Every field (recursively) is integer/pointer-shaped — the struct
    /// travels in GPRs, and at ≤ 8 B that is ONE GPR: modellable.
    AllIntegral,
    /// At least one floating-point field — HFA territory (v-registers),
    /// or a mixed composite whose GPR-vs-SIMD split needs layout
    /// analysis. Refused either way in v1; the message carries the field.
    HasFp(String),
    /// A field whose class cannot be established from the DB — refuse
    /// rather than guess; the message carries the field.
    Unknown(String),
}

/// Classify a scalar-kind row — the half of the classifier that needs no
/// field walking, kept as a pure function so it is testable without any
/// database at all (the place where a wrong answer becomes a silently
/// mis-marshalled call rather than an error).
///
/// `kind`/`type_name` come straight from the `types` table. `size_bits`
/// is `NULL` for primitives, so classification is by NAME for those —
/// exact, since the primitive set is closed. (Measured caveat for this DB
/// build: `kind='primitive'` is polluted with nested-anonymous
/// struct/union placeholder names like `_Anonymous_e__Union`; the
/// whitelist below lets exactly the real primitives through and refuses
/// the placeholders, which is the correct reading — a placeholder may
/// hold anything, doubles included.)
fn classify_scalar(kind: &str, type_name: &str) -> Option<Result<ArgClass, WinkbError>> {
    // An array type decays to a pointer in PARAMETER position, whatever
    // its element type — `f64[]` is a POINTER argument, not a float one.
    // Checked before the primitive names below, which it would otherwise
    // match by prefix.
    if type_name.ends_with("[]") {
        return Some(Ok(ArgClass::G));
    }
    match kind {
        // A pointer is a pointer regardless of pointee; `interface` is a
        // COM interface pointer; `reference` an array/由-ref view.
        "pointer" | "reference" | "interface" => Some(Ok(ArgClass::G)),
        // WINARM (P5, win_gui_design.md §2.2): a `delegate` is a callback
        // FUNCTION POINTER — a code address in a GPR, nothing more. WG
        // passes Rust trampoline addresses through these (RegisterClassW's
        // wndproc; SetTimer's TIMERPROC), with no Smalltalk-side callback
        // marshalling, so the refusal stance must not catch it: it is
        // modelled trivially as `g`.
        "delegate" => Some(Ok(ArgClass::G)),
        // An enum's underlying type is always integral (8/16/32/64-bit
        // rows all exist in this build; every one fits a GPR).
        "enum" => Some(Ok(ArgClass::G)),
        "primitive" => Some(match type_name {
            "f64" => Ok(ArgClass::F),
            // WINARM (P5): re-derived AWAY from WINVM, which classified
            // f32 → F. The marshal shape (`runtime::ffi::marshal_f`) is
            // `f64::to_bits` into a full d-register; an f32 callee reads
            // the S-register — the LOW 32 BITS — which under that shape
            // hold the mantissa tail of the double, i.e. garbage, with no
            // fault anywhere. The exact bug class this module exists to
            // refuse. (Real exemplar pinned in tests: `D2D1Vec3Length`.)
            "f32" => Err(WinkbError::Unsupported(
                "f32 travels in the s-register (low half of the v-register) on ARM64, and \
                 the FFI marshal shape is f64 bits in the full d-register — an f32 through \
                 it would silently pass garbage; f32 is refused until a narrowing marshal \
                 exists (winkb D2)"
                    .into(),
            )),
            "void" => Ok(ArgClass::V),
            "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize"
            | "bool" | "char" | "string" => Ok(ArgClass::G),
            other => Err(WinkbError::Unsupported(format!(
                "primitive type `{other}` has no FFI register class"
            ))),
        }),
        // `struct` needs the field walk — not scalar territory.
        _ => None,
    }
}

/// Classify one `types` row for THIS target. Struct rows consult
/// `struct_fields` (recursively, via `field_content`) because MS ARM64's
/// composite rules hinge on floating-point content — see the module-doc
/// table. `depth` caps nested-struct recursion.
#[cfg(windows)]
fn classify_type(
    conn: &rusqlite::Connection,
    type_id: i64,
    depth: u32,
) -> Result<ArgClass, WinkbError> {
    let (kind, tname, bits): (String, String, Option<i64>) = conn
        .query_row(
            "SELECT kind, type_name, size_bits FROM types WHERE type_id = ?1",
            [type_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| WinkbError::Db(e.to_string()))?;

    if let Some(res) = classify_scalar(&kind, &tname) {
        return res;
    }
    match kind.as_str() {
        "struct" => classify_struct(conn, type_id, &tname, bits, depth),
        other => Err(WinkbError::Unsupported(format!(
            "type `{tname}` of kind `{other}` has no FFI register class"
        ))),
    }
}

/// The composite rules, re-derived for MS ARM64 (D2). Every refusal names
/// the ABI rule it fell to — these messages are guest-visible and are the
/// difference between a diagnosis and a garbage call.
#[cfg(windows)]
fn classify_struct(
    conn: &rusqlite::Connection,
    type_id: i64,
    type_name: &str,
    size_bits: Option<i64>,
    depth: u32,
) -> Result<ArgClass, WinkbError> {
    let bits = match size_bits {
        Some(b) if b > 0 => b,
        // 2,554 rows in this build are field-less, size-0 COM class
        // objects (DOMDocument and friends) — nothing to model.
        _ => {
            return Err(WinkbError::Unsupported(format!(
                "struct `{type_name}` has no recorded size — cannot be classified"
            )))
        }
    };
    match field_content(conn, type_id, depth)? {
        FieldContent::HasFp(field) => Err(WinkbError::Unsupported(format!(
            "struct `{type_name}` has a floating-point field (`{field}`): on MS ARM64 a \
             homogeneous float aggregate travels in v0–v3 (s/d registers), NOT a GPR — \
             D2D_POINT_2F-shaped 8-byte structs land in s0/s1 where a `g` word would put \
             an x-register — and telling HFA from mixed needs layout analysis v1 does not \
             do; refused (winkb D2, HFA row)"
        ))),
        FieldContent::Unknown(field) => Err(WinkbError::Unsupported(format!(
            "struct `{type_name}`'s field `{field}` has no modelable register class — \
             refusing rather than guessing (winkb D2)"
        ))),
        FieldContent::AllIntegral => {
            if bits <= 64 {
                // One GPR, by value — how HANDLE, HWND, BOOL, PSTR,
                // POINT and FILETIME actually travel.
                Ok(ArgClass::G)
            } else if bits <= 128 {
                Err(WinkbError::Unsupported(format!(
                    "struct `{type_name}` is {} bytes: MS ARM64 passes it in TWO GPRs \
                     (x0:x1 for returns), and the FFI's one-token-per-argument g/f \
                     vocabulary cannot express a two-register composite — refused in v1; \
                     the two-GPR ABI fact itself is execution-pinned by \
                     it_codecache::struct16_return_in_x0_x1 (winkb D2)",
                    bits / 8
                )))
            } else {
                Err(WinkbError::Unsupported(format!(
                    "struct `{type_name}` is {} bytes (> 16): MS ARM64 passes it \
                     INDIRECTLY — returns via the caller-allocated x8 pointer, parameters \
                     by reference — neither modelled in v1 (the Win-x64 port's \
                     hidden-pointer bug lived exactly here: a pointer plus garbage with \
                     nothing failing at the point of error). Pass a `g` POINTER to \
                     caller-owned memory instead (winkb D2)",
                    bits / 8
                )))
            }
        }
    }
}

/// Walk a struct's recorded fields answering their floating-point
/// content. `struct_fields.type_id` is fully populated in this build
/// (measured: 0 NULLs of 66,708), so nested fields resolve by kind;
/// primitive-kind fields classify by the closed name set. Depth-capped:
/// a struct small enough to matter (≤ 16 B) nests shallowly.
#[cfg(windows)]
fn field_content(
    conn: &rusqlite::Connection,
    struct_type_id: i64,
    depth: u32,
) -> Result<FieldContent, WinkbError> {
    if depth > 4 {
        return Ok(FieldContent::Unknown("<nesting deeper than 4>".into()));
    }
    let mut stmt = conn
        .prepare(
            "SELECT field_name, type_name, type_id FROM struct_fields \
             WHERE struct_type_id = ?1 ORDER BY ordinal",
        )
        .map_err(|e| WinkbError::Db(e.to_string()))?;
    let rows: Vec<(String, String, Option<i64>)> = stmt
        .query_map([struct_type_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| WinkbError::Db(e.to_string()))?
        .collect::<Result<_, _>>()
        .map_err(|e| WinkbError::Db(e.to_string()))?;

    if rows.is_empty() {
        return Ok(FieldContent::Unknown("<no recorded fields>".into()));
    }

    for (fname, ftname, ftid) in rows {
        // Inline float arrays are FP content whatever their length —
        // `f32[2]` IS the classic HFA layout.
        if ftname == "f32" || ftname == "f64" || ftname == "f32[]" || ftname == "f64[]" {
            return Ok(FieldContent::HasFp(fname));
        }
        let Some(ftid) = ftid else {
            return Ok(FieldContent::Unknown(fname));
        };
        let (fkind, ftype_name): (String, String) = conn
            .query_row(
                "SELECT kind, type_name FROM types WHERE type_id = ?1",
                [ftid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| WinkbError::Db(e.to_string()))?;
        match fkind.as_str() {
            // Address-shaped or integral whatever the details.
            "pointer" | "interface" | "delegate" | "enum" => {}
            "primitive" => match ftype_name.as_str() {
                "f32" | "f64" | "f32[]" | "f64[]" => return Ok(FieldContent::HasFp(fname)),
                "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize"
                | "bool" | "char" | "string" | "u8[]" | "u16[]" | "u32[]" | "u64[]" | "i8[]"
                | "i16[]" | "i32[]" | "i64[]" | "usize[]" | "isize[]" | "char[]" | "void*[]" => {}
                // The `_Anonymous_e__Union`-style placeholders land here:
                // an anonymous union may hold anything, doubles included.
                _ => return Ok(FieldContent::Unknown(fname)),
            },
            "struct" => match field_content(conn, ftid, depth + 1)? {
                FieldContent::AllIntegral => {}
                FieldContent::HasFp(inner) => {
                    return Ok(FieldContent::HasFp(format!("{fname}.{inner}")))
                }
                FieldContent::Unknown(inner) => {
                    return Ok(FieldContent::Unknown(format!("{fname}.{inner}")))
                }
            },
            "reference" => {
                // An array-view field: FP-ness follows the element name,
                // which for references is textual (`CHAR[]`, `FOO[]`).
                if ftype_name.starts_with("f32") || ftype_name.starts_with("f64") {
                    return Ok(FieldContent::HasFp(fname));
                }
            }
            _ => return Ok(FieldContent::Unknown(fname)),
        }
    }
    Ok(FieldContent::AllIntegral)
}

// ── Function lookup ─────────────────────────────────────────────────────────

/// Resolve an exported function by exact name.
///
/// Windows text APIs come in `A`/`W` pairs (`CreateFileA`/`CreateFileW`)
/// and this does no aliasing: the caller names the exact export it wants,
/// because picking one silently is precisely the sort of guess this
/// module exists to avoid.
#[cfg(windows)]
pub fn lookup_function(name: &str) -> Result<FnSig, WinkbError> {
    let conn = open()?;
    lookup_function_conn(&conn, name)
}

#[cfg(windows)]
fn lookup_function_conn(conn: &rusqlite::Connection, name: &str) -> Result<FnSig, WinkbError> {
    let (fid, dll, callconv, is_variadic, ret_type): (
        i64,
        Option<String>,
        Option<String>,
        i64,
        Option<i64>,
    ) = conn
        .query_row(
            "SELECT function_id, dll_name, callconv, is_variadic, return_type_id
               FROM functions WHERE function_name = ?1 LIMIT 1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => WinkbError::NotFound(name.to_string()),
            other => WinkbError::Db(other.to_string()),
        })?;

    let dll = dll.ok_or_else(|| {
        WinkbError::Unsupported(format!("{name} has no DLL recorded — cannot be resolved"))
    })?;

    // WINARM (P5): `extern "system"` ≡ `extern "C"` on ARM64 (no stdcall
    // legacy), so the convention column is not a dispatch input — but it
    // is asserted cheaply on read, per the sprint doc's pitfall list, so
    // an x86-flavoured row leaking through a future query/schema refuses
    // instead of being called under a silently wrong assumption. Measured
    // population of this build: 16,897 "winapi" + 1,374 "cdecl", nothing
    // else — both are AAPCS64 here.
    match callconv.as_deref() {
        None | Some("winapi") | Some("cdecl") => {}
        Some(other) => {
            return Err(WinkbError::Unsupported(format!(
                "{name}: calling convention `{other}` is not an ARM64 convention — an x86 \
                 row leaked through the query? (winkb: extern \"system\" ≡ extern \"C\" \
                 here; only winapi/cdecl rows are callable)"
            )))
        }
    }

    // The variadic booby-trap row (D2), refused loudly — see the module
    // doc's Δ for why this belt rarely gets to fire from THIS build's
    // data and what guards the hole instead.
    if is_variadic != 0 {
        return Err(WinkbError::Unsupported(format!(
            "{name} is VARIADIC: MS ARM64 passes every variadic argument in GPRs — floats \
             bit-copied into x-registers — while these trampolines place floats in \
             d-registers and integer args 9+ per the fixed-arity convention (Apple puts \
             variadics on the stack). Calling through them would mis-place arguments \
             silently; variadic imports are refused in v1 (winkb D2)"
        )));
    }

    let ret = match ret_type {
        Some(t) => classify_type(conn, t, 0)
            .map_err(|e| prefix_unsupported(e, &format!("{name} return value")))?,
        None => ArgClass::V,
    };

    let mut stmt = conn
        .prepare("SELECT type_id FROM function_params WHERE function_id = ?1 ORDER BY ordinal")
        .map_err(|e| WinkbError::Db(e.to_string()))?;
    let ids: Vec<i64> = stmt
        .query_map([fid], |r| r.get(0))
        .map_err(|e| WinkbError::Db(e.to_string()))?
        .collect::<Result<_, _>>()
        .map_err(|e| WinkbError::Db(e.to_string()))?;

    let mut params = Vec::with_capacity(ids.len());
    for (i, tid) in ids.iter().enumerate() {
        let c = classify_type(conn, *tid, 0)
            .map_err(|e| prefix_unsupported(e, &format!("{name} argument {i}")))?;
        if c == ArgClass::V {
            return Err(WinkbError::Unsupported(format!(
                "{name} argument {i} is void"
            )));
        }
        params.push(c);
    }

    // The trampoline buffers are the hard ceiling: 16 g-slots (8 register
    // + 8 stack-spill) and 8 f-slots (`codecache::ffi_stubs`).
    let g_count = params.iter().filter(|c| **c == ArgClass::G).count();
    let f_count = params.iter().filter(|c| **c == ArgClass::F).count();
    if g_count > crate::codecache::ffi_stubs::ARGV_G_WORDS {
        return Err(WinkbError::Unsupported(format!(
            "{name} takes {g_count} integer-class arguments, more than the {} the \
             trampoline buffer holds",
            crate::codecache::ffi_stubs::ARGV_G_WORDS
        )));
    }
    if f_count > 8 {
        return Err(WinkbError::Unsupported(format!(
            "{name} takes {f_count} float-class arguments, more than the 8 FPR slots the \
             trampoline loads"
        )));
    }

    Ok(FnSig {
        name: name.to_string(),
        dll,
        ret,
        params,
        is_variadic: is_variadic != 0,
    })
}

/// Keep [`WinkbError::Unsupported`] messages self-locating ("{fn} argument
/// {i}: …") without flattening the other variants.
#[cfg(windows)]
fn prefix_unsupported(e: WinkbError, what: &str) -> WinkbError {
    match e {
        WinkbError::Unsupported(m) => WinkbError::Unsupported(format!("{what}: {m}")),
        other => other,
    }
}

// ── COM lookup (data only in P5 — the dispatch arm is win_gui's) ───────────

/// Resolve a COM interface method to its vtable slot.
///
/// The Windows counterpart of MACVM's Tier-2 `objc_msgSend` dispatch:
/// given an interface and a method name, the two facts needed to call it
/// dynamically are the vtable index and the argument classes. `params`
/// includes the implicit `this` at position 0. P5 models and pins these
/// FACTS (the D2 COM row); no dispatch arm consumes them yet — that is
/// the `win_gui` track's, riding this resolver (sprint doc "Interfaces
/// for later sprints").
#[cfg(windows)]
pub fn lookup_com_method(interface: &str, method: &str) -> Result<ComMethod, WinkbError> {
    let conn = open()?;
    let (tid, iid): (i64, Option<String>) = conn
        .query_row(
            "SELECT type_id, iid FROM types WHERE type_name = ?1 AND kind = 'interface' LIMIT 1",
            [interface],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => WinkbError::NotFound(interface.to_string()),
            other => WinkbError::Db(other.to_string()),
        })?;
    let iid = iid.ok_or_else(|| {
        WinkbError::Unsupported(format!("interface {interface} has no IID recorded"))
    })?;

    let (mid, vtable_index, ret_name): (i64, i64, Option<String>) = conn
        .query_row(
            "SELECT method_id, vtable_index, return_type_name
               FROM interface_methods WHERE interface_type_id = ?1 AND method_name = ?2 LIMIT 1",
            rusqlite::params![tid, method],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                WinkbError::NotFound(format!("{interface}::{method}"))
            }
            other => WinkbError::Db(other.to_string()),
        })?;

    // Interface method params carry type NAMES rather than ids. Every COM
    // method returns HRESULT or a handle-like value and takes pointers or
    // integers, so a name-based classification is adequate for the DATA
    // model — but float parameters must still be recognised, not assumed
    // away, and on THIS target f32 refuses exactly as in the scalar
    // classifier (same silent-garbage hazard, same rule).
    let class_of_name = |n: &str| -> Result<ArgClass, WinkbError> {
        match n {
            "f64" => Ok(ArgClass::F),
            "f32" => Err(WinkbError::Unsupported(format!(
                "{interface}::{method}: f32 parameter/return — refused on ARM64 (s-register \
                 width; see winkb D2)"
            ))),
            "void" => Ok(ArgClass::V),
            _ => Ok(ArgClass::G),
        }
    };

    let mut stmt = conn
        .prepare(
            "SELECT type_name FROM interface_method_params WHERE method_id = ?1 ORDER BY ordinal",
        )
        .map_err(|e| WinkbError::Db(e.to_string()))?;
    let names: Vec<String> = stmt
        .query_map([mid], |r| r.get(0))
        .map_err(|e| WinkbError::Db(e.to_string()))?
        .collect::<Result<_, _>>()
        .map_err(|e| WinkbError::Db(e.to_string()))?;

    // `this` first — the call really does pass it, so the classes must
    // line up with what a trampoline would marshal.
    let mut params = vec![ArgClass::G];
    for n in &names {
        params.push(class_of_name(n)?);
    }

    Ok(ComMethod {
        interface: interface.to_string(),
        iid,
        method: method.to_string(),
        vtable_index,
        params,
        ret: match ret_name.as_deref() {
            Some(n) => class_of_name(n)?,
            None => ArgClass::G,
        },
    })
}

// ── Constants and struct fields (taken from WINVM verbatim; schema verified) ─

/// Look up a named constant (`GENERIC_READ`, `OPEN_EXISTING`, …).
///
/// Searches `enum_members` FIRST and `constants` second, because that is
/// where the constants callers actually name live: `GENERIC_READ`,
/// `FILE_SHARE_READ` and `OPEN_EXISTING` are all flag-enum members, and
/// the `constants` table holds a different population (SDK
/// `#define`-style values). Returned as `u64` because the common flags
/// are unsigned and several have the top bit set — `GENERIC_READ` is
/// `0x8000_0000`. The `signedness` column decides which stored
/// representation is authoritative.
#[cfg(windows)]
pub fn lookup_constant(name: &str) -> Result<u64, WinkbError> {
    let conn = open()?;

    let from_enum = conn.query_row(
        "SELECT value_i64, value_u64, signedness
           FROM enum_members WHERE member_name = ?1 LIMIT 1",
        [name],
        |r| {
            let i: i64 = r.get(0)?;
            let u: i64 = r.get(1)?;
            let sign: Option<String> = r.get(2)?;
            Ok(match sign.as_deref() {
                Some("signed") => i as u64,
                _ => u as u64,
            })
        },
    );
    match from_enum {
        Ok(v) => return Ok(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => {}
        Err(e) => return Err(WinkbError::Db(e.to_string())),
    }

    conn.query_row(
        "SELECT value_i64, value_u64, value_kind
           FROM constants WHERE constant_name = ?1 LIMIT 1",
        [name],
        |r| {
            let i: Option<i64> = r.get(0)?;
            let u: Option<i64> = r.get(1)?;
            let kind: Option<String> = r.get(2)?;
            Ok(match kind.as_deref() {
                Some(k) if k.starts_with("int") => i.unwrap_or(0) as u64,
                _ => u.or(i).unwrap_or(0) as u64,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => WinkbError::NotFound(name.to_string()),
        other => WinkbError::Db(other.to_string()),
    })
}

/// A struct's total size in BYTES — the other half of "build a struct by
/// layout", added by WG0 (`docs/sprints/sprint_wg0_detail.md` D1 step 4).
///
/// [`lookup_struct_field`] alone tells a caller where to WRITE; it never
/// says how much to ALLOCATE, and guessing that from the last field's
/// offset silently drops tail padding. The two facts also live in
/// independent columns (`types.size_bits` vs `struct_fields.byte_offset`),
/// which makes "size == last offset + last field's width" a real
/// cross-check on the database rather than a tautology — WG0's struct test
/// asserts exactly that instead of re-transcribing header numbers.
///
/// `size_bits` is the recorded bit width; a struct whose size is missing or
/// not a whole number of bytes is [`WinkbError::Unsupported`] rather than a
/// rounded guess (the module's founding stance).
#[cfg(windows)]
pub fn lookup_struct_size(struct_name: &str) -> Result<i64, WinkbError> {
    let conn = open()?;
    let bits: Option<i64> = conn
        .query_row(
            "SELECT size_bits FROM types WHERE type_name = ?1 AND kind = 'struct' LIMIT 1",
            [struct_name],
            |r| r.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => WinkbError::NotFound(struct_name.to_string()),
            other => WinkbError::Db(other.to_string()),
        })?;
    match bits {
        Some(b) if b > 0 && b % 8 == 0 => Ok(b / 8),
        other => Err(WinkbError::Unsupported(format!(
            "struct `{struct_name}` records size_bits = {other:?} — not a whole number of \
             bytes, so its size cannot be answered (winkb)"
        ))),
    }
}

/// A struct field's byte offset — what `Alien` needs to read a field by
/// name instead of a hand-counted constant. (The world's Windows clock
/// branch pins `TIME_ZONE_INFORMATION`'s offsets against this — see the
/// real-DB test below.)
#[cfg(windows)]
pub fn lookup_struct_field(struct_name: &str, field: &str) -> Result<i64, WinkbError> {
    let conn = open()?;
    conn.query_row(
        "SELECT sf.byte_offset
           FROM struct_fields sf JOIN types t ON t.type_id = sf.struct_type_id
          WHERE t.type_name = ?1 AND sf.field_name = ?2 LIMIT 1",
        rusqlite::params![struct_name, field],
        |r| r.get(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            WinkbError::NotFound(format!("{struct_name}.{field}"))
        }
        other => WinkbError::Db(other.to_string()),
    })
}

// ── resolve_export: the dlsym twin (LoadLibraryA + GetProcAddress) ──────────

/// The modules probed for a `lib: None` lookup — Windows has no
/// `RTLD_DEFAULT` "search everything loaded". The list is the closest
/// analogue for the names hand-authored pragmas actually use:
///
/// - the CRT pair first (Tier-1 world bindings are libc-shaped);
/// - `kernel32` (the Win32 core);
/// - `user32` — WINARM (P5, win_gui_design.md): WG0's first calls
///   (`MessageBeep`, `GetSystemMetrics`) must resolve even with no DB;
/// - `ws2_32` — the D5 Δ's DNS trio (`getaddrinfo`/`freeaddrinfo`/
///   `inet_ntop`) lives there, and keeping it in the probe keeps the
///   DB-present and DB-absent worlds behaviourally identical (tests_p05
///   gate item 5).
#[cfg(windows)]
const DEFAULT_PROBE: &[&str] = &[
    "ucrtbase.dll",
    "msvcrt.dll",
    "kernel32.dll",
    "user32.dll",
    "ws2_32.dll",
];

// The three kernel32 imports of the resolution helper itself, declared by
// hand (`extern "system"`, the P0 convention: `libc` on Windows exposes
// the CRT only, and the core crate takes no `windows-sys` dependency).
// Declared HERE, not in the vendored loader: P1's Δ records that
// `vendor/wfasm/native_winarm64.rs` deliberately carries only the JIT
// memory surface, and this helper's consumers are all runtime-side FFI.
#[cfg(windows)]
extern "system" {
    fn LoadLibraryA(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn GetModuleHandleA(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn GetProcAddress(
        module: *mut std::ffi::c_void,
        name: *const std::ffi::c_char,
    ) -> *mut std::ffi::c_void;
}

/// Resolve one native symbol to its absolute runtime address — the
/// Windows twin of `native_macos::dlsym_resolve`, shaped like WINVM's.
/// `Some(dll)` loads/finds that module; `None` probes [`DEFAULT_PROBE`].
///
/// The MSVC CRT exports POSIX names with a leading underscore (`getpid` →
/// `_getpid`, `open` → `_open`, …); the plain name is tried first, then
/// the underscore alias, so Tier-1 world bindings written against POSIX
/// names keep working unchanged — the same call P0 made when it renamed
/// the Rust-side `getpid` link rather than gating the test away.
/// `LoadLibraryA` on an already-loaded module is cheap (refcount + same
/// handle); resolution results are cached in the FFI descriptor anyway
/// (slot 6), so this path runs once per binding.
// The crate denies `unsafe_code`; this function is one of the enumerated
// OS-seam exceptions (same boundary rationale as `memory/reservation.rs`'s
// win module and `vendor/wfasm/native_winarm64.rs`): a raw
// LoadLibrary/GetProcAddress call has no safe-Rust equivalent, every call
// is null-checked, and no pointer outlives this function.
#[allow(unsafe_code)]
#[cfg(windows)]
pub fn resolve_export(lib: Option<&str>, symbol: &str) -> Option<u64> {
    use std::ffi::CString;
    let c_sym = CString::new(symbol).ok()?;
    let c_sym_underscore = CString::new(format!("_{symbol}")).ok()?;
    let lookup = |handle: *mut std::ffi::c_void| {
        // SAFETY: `handle` is a live module handle; both names are valid
        // NUL-terminated C strings. GetProcAddress reads no memory beyond
        // them.
        unsafe {
            let addr = GetProcAddress(handle, c_sym.as_ptr());
            let addr = if addr.is_null() {
                GetProcAddress(handle, c_sym_underscore.as_ptr())
            } else {
                addr
            };
            if addr.is_null() {
                None
            } else {
                Some(addr as u64)
            }
        }
    };
    let module = |name: &str| {
        let c_mod = CString::new(name).ok()?;
        // SAFETY: valid C string. GetModuleHandleA first (no refcount
        // bump for already-loaded modules), LoadLibraryA as the fallback
        // for one not yet mapped; both results are null-checked.
        let h = unsafe {
            let h = GetModuleHandleA(c_mod.as_ptr());
            if h.is_null() {
                LoadLibraryA(c_mod.as_ptr())
            } else {
                h
            }
        };
        if h.is_null() {
            None
        } else {
            Some(h)
        }
    };
    match lib {
        Some(path) => lookup(module(path)?),
        None => DEFAULT_PROBE.iter().find_map(|m| lookup(module(m)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scalar half of the classifier is pure and testable without any
    /// database — the place where a wrong answer becomes a silently
    /// mis-marshalled call rather than an error.
    #[test]
    fn scalar_classification_covers_the_cases_that_decide_a_call() {
        let ok = |k: &str, n: &str| classify_scalar(k, n).expect("scalar").expect("class");
        assert_eq!(ok("primitive", "f64"), ArgClass::F);
        assert_eq!(ok("primitive", "u32"), ArgClass::G);
        assert_eq!(ok("primitive", "i64"), ArgClass::G);
        assert_eq!(ok("primitive", "usize"), ArgClass::G);
        assert_eq!(ok("primitive", "void"), ArgClass::V);
        assert_eq!(ok("pointer", "FOO*"), ArgClass::G);
        assert_eq!(ok("enum", "FILE_SHARE_MODE"), ArgClass::G);
        // WINARM (P5, win_gui_design.md §2.2): a callback function
        // pointer is a plain g-class pointer — modelled, never refused.
        assert_eq!(ok("delegate", "WNDPROC"), ArgClass::G);
        assert_eq!(ok("delegate", "TIMERPROC"), ArgClass::G);
        // An ARRAY of doubles is a pointer parameter, not a float one.
        assert_eq!(ok("primitive", "f64[]"), ArgClass::G);

        // f32 REFUSES on this target (s-register width vs the f64-bits
        // marshal shape) — the re-derivation away from WINVM's x64 rule.
        let e = classify_scalar("primitive", "f32").unwrap().unwrap_err();
        assert!(
            matches!(e, WinkbError::Unsupported(ref m) if m.contains("s-register")),
            "f32 must refuse naming the s-register hazard, got {e:?}"
        );
        // The placeholder names polluting kind='primitive' refuse too.
        let e = classify_scalar("primitive", "_Anonymous_e__Union")
            .unwrap()
            .unwrap_err();
        assert!(matches!(e, WinkbError::Unsupported(_)));

        // Structs are NOT scalar territory — they need the field walk.
        assert!(classify_scalar("struct", "HANDLE").is_none());
    }

    #[test]
    fn class_mask_marks_float_positions() {
        let sig = FnSig {
            name: "f".into(),
            dll: "x.dll".into(),
            ret: ArgClass::G,
            params: vec![ArgClass::G, ArgClass::F, ArgClass::G, ArgClass::F],
            is_variadic: false,
        };
        assert_eq!(sig.class_mask(), 0b1010);
    }

    /// `db_path`'s default is this machine's delivered copy; `WINKB_DB`
    /// remains the override (asserted structurally, not by mutating the
    /// process environment — parallel tests share it).
    #[test]
    fn db_path_defaults_to_the_delivered_artifact() {
        if std::env::var("WINKB_DB").map_or(true, |v| v.is_empty()) {
            assert_eq!(
                db_path(),
                PathBuf::from(r"C:\projects\windows_api\windows_api.db")
            );
        }
    }

    /// The founding contract: a missing database is `DbMissing`, never a
    /// panic and never a fabricated answer — the caller's pragma fallback
    /// (and the no-DB CI story) rides on this exact variant.
    #[cfg(windows)]
    #[test]
    fn db_missing_is_not_an_error() {
        let ghost =
            std::env::temp_dir().join(format!("winkb_no_such_db_{}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&ghost);
        match open_at(&ghost) {
            Err(WinkbError::DbMissing(p)) => assert_eq!(p, ghost),
            other => panic!("expected DbMissing, got {other:?}"),
        }
    }

    // ── The fixture DB: deliberately-wrong rows must REFUSE, not call ──
    // (tests_p05.md stress row: "the DB is data, not gospel".)

    #[cfg(windows)]
    fn fixture_db() -> (std::path::PathBuf, rusqlite::Connection) {
        let p = std::env::temp_dir().join(format!(
            "winkb_fixture_{}_{:?}.sqlite3",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&p);
        let conn = rusqlite::Connection::open(&p).expect("create fixture");
        conn.execute_batch(
            "CREATE TABLE types (type_id INTEGER PRIMARY KEY, type_name TEXT, kind TEXT,
                                 size_bits INTEGER);
             CREATE TABLE functions (function_id INTEGER PRIMARY KEY, function_name TEXT,
                                     dll_name TEXT, callconv TEXT, is_variadic INTEGER,
                                     return_type_id INTEGER);
             CREATE TABLE function_params (function_id INTEGER, ordinal INTEGER,
                                           type_id INTEGER);
             CREATE TABLE struct_fields (struct_type_id INTEGER, ordinal INTEGER,
                                         field_name TEXT, type_name TEXT, type_id INTEGER,
                                         byte_offset INTEGER);
             -- types
             INSERT INTO types VALUES (1, 'i32',  'primitive', NULL);
             INSERT INTO types VALUES (2, 'f32',  'primitive', NULL);
             INSERT INTO types VALUES (3, 'f64',  'primitive', NULL);
             INSERT INTO types VALUES (4, 'void*','pointer',   NULL);
             INSERT INTO types VALUES (5, 'FAKEPROC', 'delegate', 64);
             -- an FP-bearing 8-byte struct (the D2D_POINT_2F shape)
             INSERT INTO types VALUES (6, 'FAKE_POINT_2F', 'struct', 64);
             INSERT INTO struct_fields VALUES (6, 0, 'x', 'f32', 2, 0);
             INSERT INTO struct_fields VALUES (6, 1, 'y', 'f32', 2, 4);
             -- an FP-free 12-byte struct (two-GPR territory)
             INSERT INTO types VALUES (7, 'FAKE_TRIPLE', 'struct', 96);
             INSERT INTO struct_fields VALUES (7, 0, 'a', 'i32', 1, 0);
             INSERT INTO struct_fields VALUES (7, 1, 'b', 'i32', 1, 4);
             INSERT INTO struct_fields VALUES (7, 2, 'c', 'i32', 1, 8);
             -- an FP-free 24-byte struct (x8-indirect territory)
             INSERT INTO types VALUES (8, 'FAKE_BIG', 'struct', 192);
             INSERT INTO struct_fields VALUES (8, 0, 'a', 'i64', 1, 0);
             INSERT INTO struct_fields VALUES (8, 1, 'b', 'i64', 1, 8);
             INSERT INTO struct_fields VALUES (8, 2, 'c', 'i64', 1, 16);
             -- a HANDLE-shaped 8-byte struct (modelled: one GPR)
             INSERT INTO types VALUES (9, 'FAKE_HANDLE', 'struct', 64);
             INSERT INTO struct_fields VALUES (9, 0, 'Value', 'void*', 4, 0);
             -- functions
             INSERT INTO functions VALUES (100, 'FakeVariadic', 'fake.dll', 'cdecl', 1, 1);
             INSERT INTO function_params VALUES (100, 0, 4);
             INSERT INTO functions VALUES (101, 'FakeX86Conv', 'fake.dll', 'stdcall', 0, 1);
             INSERT INTO functions VALUES (102, 'FakeTakesF32', 'fake.dll', 'winapi', 0, 1);
             INSERT INTO function_params VALUES (102, 0, 2);
             INSERT INTO functions VALUES (103, 'FakeTakesFpStruct', 'fake.dll', 'winapi', 0, 1);
             INSERT INTO function_params VALUES (103, 0, 6);
             INSERT INTO functions VALUES (104, 'FakeTakesTriple', 'fake.dll', 'winapi', 0, 1);
             INSERT INTO function_params VALUES (104, 0, 7);
             INSERT INTO functions VALUES (105, 'FakeReturnsBig', 'fake.dll', 'winapi', 0, 8);
             INSERT INTO functions VALUES (106, 'FakeCallback', 'fake.dll', 'winapi', 0, 1);
             INSERT INTO function_params VALUES (106, 0, 5);
             INSERT INTO function_params VALUES (106, 1, 9);
             INSERT INTO functions VALUES (107, 'FakeTakesF64', 'fake.dll', 'winapi', 0, 3);
             INSERT INTO function_params VALUES (107, 0, 3);
             INSERT INTO function_params VALUES (107, 1, 1);",
        )
        .expect("populate fixture");
        (p, conn)
    }

    #[cfg(windows)]
    #[test]
    fn fixture_rows_that_cannot_be_modelled_refuse_with_named_reasons() {
        let (path, conn) = fixture_db();
        let unsupported = |name: &str| -> String {
            match lookup_function_conn(&conn, name) {
                Err(WinkbError::Unsupported(m)) => m,
                other => panic!("{name}: expected Unsupported, got {other:?}"),
            }
        };

        // The variadic booby trap names BOTH conventions.
        let m = unsupported("FakeVariadic");
        assert!(
            m.contains("VARIADIC") && m.contains("GPRs") && m.contains("stack"),
            "{m}"
        );

        // An x86 calling-convention row refuses (extern "system" ≡
        // extern "C" assert).
        let m = unsupported("FakeX86Conv");
        assert!(m.contains("stdcall") && m.contains("ARM64"), "{m}");

        // f32 refuses naming the s-register hazard.
        let m = unsupported("FakeTakesF32");
        assert!(m.contains("s-register"), "{m}");

        // The 8-byte all-float struct refuses naming the HFA rule — the
        // exact case a size-only (Win-x64-style) classifier would have
        // silently modelled as one GPR.
        let m = unsupported("FakeTakesFpStruct");
        assert!(
            m.contains("floating-point field") && m.contains("v0"),
            "{m}"
        );

        // 9–16 B FP-free: two-GPR territory, not expressible in one token.
        let m = unsupported("FakeTakesTriple");
        assert!(m.contains("TWO GPRs"), "{m}");

        // > 16 B: the x8-indirect rule, named.
        let m = unsupported("FakeReturnsBig");
        assert!(m.contains("x8"), "{m}");

        // And the MODELLED fixture rows still resolve: a delegate param
        // is a plain g pointer (win_gui_design.md §2.2), a HANDLE-shaped
        // struct is one GPR, f64 is F.
        let sig = lookup_function_conn(&conn, "FakeCallback").expect("FakeCallback");
        assert_eq!(sig.params, vec![ArgClass::G, ArgClass::G]);
        assert_eq!(sig.class_mask(), 0);
        let sig = lookup_function_conn(&conn, "FakeTakesF64").expect("FakeTakesF64");
        assert_eq!(sig.params, vec![ArgClass::F, ArgClass::G]);
        assert_eq!(sig.ret, ArgClass::F);
        assert_eq!(sig.class_mask(), 0b01);

        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    /// Against the real database when present. Skipped, loudly, when it
    /// is not — the DB is a machine-local artifact and the suite must
    /// stay green without it (the DB-absent gate run exercises exactly
    /// that skip).
    #[cfg(windows)]
    #[test]
    fn resolves_real_win32_symbols_with_arm64_classes() {
        if !available() {
            eprintln!(
                "[winkb] SKIP resolves_real_win32_symbols_with_arm64_classes — no database at {}",
                db_path().display()
            );
            return;
        }

        // The D3 pinning set's signatures, as the classifier sees them.
        let f = lookup_function("GetTickCount64").expect("GetTickCount64");
        assert_eq!(f.dll.to_ascii_lowercase(), "kernel32.dll");
        assert_eq!(f.ret, ArgClass::G);
        assert!(f.params.is_empty());

        let f = lookup_function("QueryPerformanceCounter").expect("QueryPerformanceCounter");
        assert_eq!(f.ret, ArgClass::G, "BOOL is a 4-byte int-struct: one GPR");
        assert_eq!(f.params, vec![ArgClass::G], "i64* out-param is a pointer");

        let f = lookup_function("GetSystemTimeAsFileTime").expect("GetSystemTimeAsFileTime");
        assert_eq!(f.ret, ArgClass::V);
        assert_eq!(f.params, vec![ArgClass::G]);

        let f = lookup_function("MulDiv").expect("MulDiv");
        assert_eq!(f.ret, ArgClass::G);
        assert_eq!(f.params, vec![ArgClass::G; 3]);

        // WG0's own first calls (win_gui_design.md): user32, enum params,
        // BOOL returns — all G.
        let f = lookup_function("GetSystemMetrics").expect("GetSystemMetrics");
        assert_eq!(f.dll.to_ascii_lowercase(), "user32.dll");
        assert_eq!(
            (f.ret, f.params.as_slice()),
            (ArgClass::G, &[ArgClass::G][..])
        );
        let f = lookup_function("MessageBeep").expect("MessageBeep");
        assert_eq!(
            (f.ret, f.params.as_slice()),
            (ArgClass::G, &[ArgClass::G][..])
        );

        // A HANDLE param travels as one GPR (the FP-free ≤8 B struct row).
        let f = lookup_function("CloseHandle").expect("CloseHandle");
        assert_eq!(f.params, vec![ArgClass::G]);

        // SetTimer's TIMERPROC (delegate) parameter is a plain g pointer
        // — the WG §2.2 requirement, against the real row.
        let f = lookup_function("SetTimer").expect("SetTimer");
        assert_eq!(f.params, vec![ArgClass::G; 4]);

        // The D5 Δ's DNS trio really is in ws2_32 with G-class shapes.
        let f = lookup_function("getaddrinfo").expect("getaddrinfo");
        assert_eq!(f.dll.to_ascii_lowercase(), "ws2_32.dll");
        assert_eq!(f.params, vec![ArgClass::G; 4]);
        let f = lookup_function("freeaddrinfo").expect("freeaddrinfo");
        assert_eq!(f.ret, ArgClass::V);

        // CreateFileW: 7 integral args, mask 0 (WINVM's own exemplar).
        let f = lookup_function("CreateFileW").expect("CreateFileW");
        assert_eq!(f.params.len(), 7);
        assert!(f.params.iter().all(|c| *c == ArgClass::G));
        assert_eq!(f.class_mask(), 0);

        // f32 surfaces REFUSE against the real DB: D2D1Vec3Length is
        // (f32,f32,f32) — WINVM's x64 classifier would have modelled it
        // and the marshal would have passed garbage.
        match lookup_function("D2D1Vec3Length") {
            Err(WinkbError::Unsupported(m)) => assert!(m.contains("s-register"), "{m}"),
            other => panic!("expected f32 refusal, got {other:?}"),
        }
        // D2D1MakeRotateMatrix refuses too: its param list is
        // (f32, D2D_POINT_2F-by-value, matrix*) in this build — param 0's
        // f32 refusal fires first in position order; the FP-STRUCT rule
        // (D2D_POINT_2F: 8 bytes of two floats, the HFA hazard a
        // size-only Win-x64 classifier would model as one GPR) is pinned
        // by the fixture test's FakeTakesFpStruct through the identical
        // query path. Either way: refused with a D2 reason, never called.
        match lookup_function("D2D1MakeRotateMatrix") {
            Err(WinkbError::Unsupported(m)) => assert!(
                m.contains("s-register") || m.contains("floating-point field"),
                "{m}"
            ),
            other => panic!("expected an f32/FP-struct refusal, got {other:?}"),
        }

        // Δ evidence, pinned so a future DB rebuild that starts carrying
        // variadic marks flips this test and upgrades the belt to live:
        // wsprintfA is genuinely variadic, yet this build records it
        // fixed-arity with only its two fixed params.
        let f = lookup_function("wsprintfA").expect("wsprintfA resolves in this build");
        assert!(
            !f.is_variadic,
            "is_variadic became populated — revisit the module-doc Δ"
        );
        assert_eq!(f.params.len(), 2);

        // COM: the two facts dynamic dispatch needs (D2's COM row).
        let m = lookup_com_method("IDXGIFactory", "CreateSwapChain").expect("IDXGIFactory");
        assert_eq!(m.iid, "7b7166ec-21c7-44ae-b21a-c9ae321ae369");
        assert_eq!(m.vtable_index, 10, "inherited methods counted: IUnknown 0–2, IDXGIObject 3–6, IDXGIFactory's own start at 7");
        assert_eq!(m.params.first(), Some(&ArgClass::G), "this pointer first");

        // Constants live in enum_members (WINVM's hard-won fact).
        assert_eq!(lookup_constant("GENERIC_READ").unwrap(), 0x8000_0000);
        assert_eq!(lookup_constant("FILE_SHARE_READ").unwrap(), 1);

        // Struct field offsets: WINVM's exemplar plus the three the
        // world's Windows clock branch (world/81) hardcodes — if the DB
        // and the world ever disagree, THIS is where it surfaces.
        assert_eq!(
            lookup_struct_field("BITMAPINFOHEADER", "biWidth").unwrap(),
            4
        );
        assert_eq!(
            lookup_struct_field("BITMAPINFOHEADER", "biBitCount").unwrap(),
            14
        );
        assert_eq!(
            lookup_struct_field("TIME_ZONE_INFORMATION", "Bias").unwrap(),
            0
        );
        assert_eq!(
            lookup_struct_field("TIME_ZONE_INFORMATION", "StandardBias").unwrap(),
            84
        );
        assert_eq!(
            lookup_struct_field("TIME_ZONE_INFORMATION", "DaylightBias").unwrap(),
            168
        );

        // WINARM (WG0): the struct WG0 actually builds, and the size query
        // added for it. The two facts come from independent columns, so
        // this pair is a real cross-check: `WNDCLASSW` is 72 bytes and its
        // last member is an 8-byte PWSTR at 64. `lpfnWndProc` at 8 is the
        // landmark the sprint's own test asserts guest-side (a 4-byte
        // `style` plus 4 bytes of padding before the first pointer — the
        // only offset in the struct derivable from first principles).
        assert_eq!(lookup_struct_size("WNDCLASSW").unwrap(), 72);
        assert_eq!(lookup_struct_field("WNDCLASSW", "lpfnWndProc").unwrap(), 8);
        assert_eq!(
            lookup_struct_field("WNDCLASSW", "lpszClassName").unwrap(),
            64
        );
        // Δ (WG0, measured): the sprint doc calls WNDCLASSW "a 16-entry
        // struct by layout". It has TEN members (WNDCLASSEXW has twelve).
        // Pinned so the number in the corrected doc has evidence behind it.
        let n: i64 = conn_count_fields("WNDCLASSW");
        assert_eq!(n, 10, "WNDCLASSW's member count (sprint_wg0_detail.md D1)");

        // A symbol that genuinely is not there must say so, not guess.
        assert!(matches!(
            lookup_function("NoSuchApiExistsW"),
            Err(WinkbError::NotFound(_))
        ));
        assert!(matches!(
            lookup_constant("NO_SUCH_CONSTANT_AT_ALL"),
            Err(WinkbError::NotFound(_))
        ));
    }

    /// How many members a struct records — WG0's member-count Δ, kept as a
    /// helper so the assertion above reads as one fact.
    #[cfg(windows)]
    fn conn_count_fields(struct_name: &str) -> i64 {
        let conn = open().expect("db");
        conn.query_row(
            "SELECT COUNT(*) FROM struct_fields sf JOIN types t ON t.type_id = sf.struct_type_id \
             WHERE t.type_name = ?1",
            [struct_name],
            |r| r.get(0),
        )
        .expect("count struct fields")
    }

    /// The dlsym twin against the real loader — no DB involved, so this
    /// runs in BOTH gate states and is the DB-absent world's resolution
    /// story in miniature.
    #[cfg(windows)]
    #[test]
    fn resolve_export_finds_crt_win32_and_underscore_aliases() {
        // Explicit library.
        assert!(resolve_export(Some("kernel32.dll"), "GetTickCount64").is_some());
        // Probe: a CRT name…
        assert!(resolve_export(None, "malloc").is_some());
        // …a POSIX name that only exists as its underscore alias
        // (`_getpid`) — the P0 posture, now in the resolver itself…
        assert!(resolve_export(None, "getpid").is_some());
        // …the WG0 user32 pair (probe list, DB-absent story)…
        assert!(resolve_export(None, "GetSystemMetrics").is_some());
        assert!(resolve_export(None, "MessageBeep").is_some());
        // …the D5 Δ's ws2_32 trio, "reclaimed by the resolver alone":
        // the SYMBOLS resolve by name with no DB — what the world tests
        // additionally need (WSAStartup lifecycle, a gai_strerror twin,
        // a non-mmap buffer) is the winsock slice's, not the resolver's.
        assert!(resolve_export(None, "getaddrinfo").is_some());
        assert!(resolve_export(None, "freeaddrinfo").is_some());
        assert!(resolve_export(None, "inet_ntop").is_some());
        // A miss stays a clean None.
        assert!(resolve_export(None, "definitely_not_a_symbol_xyzzy").is_none());
        assert!(resolve_export(Some("no_such_library_xyzzy.dll"), "malloc").is_none());
    }
}
