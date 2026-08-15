//! **WG11-W11: the MSL → HLSL dialect shim.**
//!
//! Copied from the sister Dart port — WINDARTTALK's
//! `port-win/dart_win32/gp_engine_d3d.cpp` (`GpTranslateShaderDialect` /
//! `GpTranslateMslEntry`), which faced the identical problem with the identical
//! input: the Smalltalk world is written against Metal (`GamePane>>shader:`
//! takes MSL source; galaxigans' cosmosShader opens with
//! `fract(sin(dot(...)))`), and `D3DCompile` rejects that outright with
//! `error X3004: undeclared identifier 'fract'`. Rather than port each game's
//! shader by hand, translate the handful of names that actually differ.
//! Vector types (`float2/3/4`, `float2x2`) and the bulk of the library
//! (sin/cos/floor/dot/length/normalize/clamp/step/smoothstep/pow/exp/abs/
//! min/max/saturate/atan2) are spelled identically in both, so the delta is
//! small.
//!
//! The sister port's deliberate constraints, kept to the letter:
//!
//!  * Only an identifier immediately followed by `(` is rewritten — a *call*.
//!    A variable innocently named `mix` is left alone; renaming it to `lerp`
//!    would shadow the intrinsic and break an otherwise valid shader.
//!  * `mod` is NOT mapped to `fmod`. GLSL/MSL `mod(x,y) = x - y*floor(x/y)`
//!    but HLSL `fmod` truncates, so they DISAGREE for negative operands — a
//!    silent wrong-pixel bug. It maps to a helper in [`DIALECT_PRELUDE`] with
//!    the GLSL definition.
//!  * The mapping is identity-safe for input that is ALREADY HLSL: none of
//!    `frac`/`lerp`/`rsqrt`/`ddx`/`ddy`/`atan2` appear as source tokens here,
//!    and a body with no `fragment` keyword keeps its signature untouched.

/// GLSL/MSL `mod` — floor-based, so the sign follows the DIVISOR (unlike
/// HLSL's truncating `fmod`). Overloaded for the widths a shader body may use.
pub const DIALECT_PRELUDE: &str = "\
float  gpModGl(float  x, float  y) { return x - y * floor(x / y); }\n\
float2 gpModGl(float2 x, float2 y) { return x - y * floor(x / y); }\n\
float3 gpModGl(float3 x, float3 y) { return x - y * floor(x / y); }\n\
float4 gpModGl(float4 x, float4 y) { return x - y * floor(x / y); }\n\
float2 gpModGl(float2 x, float  y) { return x - y * floor(x / y); }\n\
float3 gpModGl(float3 x, float  y) { return x - y * floor(x / y); }\n\
float4 gpModGl(float4 x, float  y) { return x - y * floor(x / y); }\n";

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Number of top-level (depth-1) comma-separated arguments of the call whose
/// `(` sits at `open`. Used to tell MSL's 2-arg `atan` from HLSL's 1-arg
/// `atan`. Returns 0 for an unbalanced call, which leaves it alone.
fn call_arg_count(s: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    let mut args = 1usize;
    for &c in &s[open..] {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return args;
                }
            }
            b',' if depth == 1 => args += 1,
            _ => {}
        }
    }
    0
}

/// Rewrite the library calls whose names differ between the dialects.
fn translate_dialect(src: &str) -> String {
    const MAP: [(&str, &str); 6] = [
        ("fract", "frac"),
        ("mix", "lerp"),
        ("inversesqrt", "rsqrt"),
        ("dfdx", "ddx"),
        ("dfdy", "ddy"),
        ("mod", "gpModGl"),
    ];
    let b = src.as_bytes();
    // Built as bytes so a stray non-ASCII byte passes through untouched; every
    // insertion is ASCII, so the result stays valid UTF-8.
    let mut out = Vec::with_capacity(src.len() + 64);
    let mut i = 0usize;
    while i < b.len() {
        if !is_ident(b[i]) || (i > 0 && is_ident(b[i - 1])) {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let mut j = i;
        while j < b.len() && is_ident(b[j]) {
            j += 1;
        }
        let tok = &src[i..j];
        // Is this a call? (identifier, optional spaces, '(')
        let mut k = j;
        while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
            k += 1;
        }
        let is_call = k < b.len() && b[k] == b'(';
        let mut rep = tok;
        if is_call {
            if let Some((_, to)) = MAP.iter().find(|(from, _)| *from == tok) {
                rep = to;
            }
            // MSL/GLSL atan(y, x) is HLSL atan2(y, x); atan(x) is atan in both.
            if tok == "atan" && call_arg_count(b, k) == 2 {
                rep = "atan2";
            }
            if tok == "discard_fragment" {
                rep = "discard";
            }
        }
        out.extend_from_slice(rep.as_bytes());
        i = j;
    }
    String::from_utf8(out).expect("ASCII-only rewrites keep UTF-8 valid")
}

/// Rewrite an MSL *fragment entry point* into the HLSL one the engine expects.
/// The world's games are written as whole Metal fragment shaders:
///
/// ```text
/// fragment float4 fmain(VOut in [[stage_in]], constant Uniforms& u [[buffer(0)]]) {
///     float t = u.time;  float2 uv = in.uv;  float a = u.aspect;  ... u.p[0] ...
/// ```
///
/// while the engine's header supplies `GVOut` (with `.uv`) and the uniforms as
/// GLOBALS (`time`, `aspect`, `p[8]`). So three things change: the `fragment`
/// qualifier and MSL `[[attributes]]` go, the parameter list collapses to
/// `(GVOut gpIn) : SV_Target`, and the uniform-struct prefix is dropped so
/// `u.time` becomes `time`. The stage_in parameter is renamed because MSL
/// bodies conventionally call it `in`, which is a parameter-modifier keyword
/// in HLSL.
///
/// Leaves a body that is already HLSL completely alone: no `fragment` token,
/// no rewrite.
fn translate_entry(src: &str) -> String {
    // 1. Drop MSL attribute clauses [[...]] wherever they appear.
    let mut s = src.to_string();
    while let Some(a) = s.find("[[") {
        match s[a..].find("]]") {
            Some(rel) => s.replace_range(a..a + rel + 2, ""),
            None => break,
        }
    }
    // 2. Find the `fragment` qualifier as a whole token.
    let b = s.as_bytes();
    let mut f = None;
    for i in 0..b.len().saturating_sub(7) {
        if &b[i..i + 8] != b"fragment" {
            continue;
        }
        if i > 0 && is_ident(b[i - 1]) {
            continue;
        }
        if i + 8 < b.len() && is_ident(b[i + 8]) {
            continue;
        }
        f = Some(i);
        break;
    }
    let Some(f) = f else {
        return s; // already HLSL — nothing to do
    };
    let Some(open) = s[f..].find('(').map(|r| f + r) else {
        return s;
    };
    let mut depth = 0usize;
    let mut close = None;
    for (i, &c) in b.iter().enumerate().skip(open) {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return s;
    };

    // 3. The entry name is the identifier just before '('.
    let mut ne = open;
    while ne > f && b[ne - 1].is_ascii_whitespace() {
        ne -= 1;
    }
    let mut nb = ne;
    while nb > f && is_ident(b[nb - 1]) {
        nb -= 1;
    }
    let fname = s[nb..ne].to_string();
    if fname.is_empty() {
        return s;
    }

    // 4. Parameter names: the uniform one is the parameter declared `constant`
    //    or by reference; the other is stage_in.
    let mut stage_in = String::new();
    let mut uniform = String::new();
    for one in s[open + 1..close].split(',') {
        let trimmed = one.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let ob = trimmed.as_bytes();
        let mut b2 = ob.len();
        while b2 > 0 && is_ident(ob[b2 - 1]) {
            b2 -= 1;
        }
        let nm = &trimmed[b2..];
        let is_uniform = trimmed.contains("constant") || trimmed.contains('&');
        if is_uniform {
            uniform = nm.to_string();
        } else if stage_in.is_empty() {
            stage_in = nm.to_string();
        }
    }

    // 5. Swap the signature for the HLSL one.
    s.replace_range(
        f..=close,
        &format!("float4 {fname}(GVOut gpIn) : SV_Target"),
    );

    // 6. `in.uv` -> `gpIn.uv`, and `u.time` -> `time` (uniforms are globals).
    let mut fixes: Vec<(String, &str)> = Vec::new();
    if !stage_in.is_empty() {
        fixes.push((format!("{stage_in}."), "gpIn."));
    }
    if !uniform.is_empty() {
        fixes.push((format!("{uniform}."), ""));
    }
    if fixes.is_empty() {
        return s;
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0usize;
    'outer: while i < b.len() {
        if i == 0 || !is_ident(b[i - 1]) {
            for (from, to) in &fixes {
                if s[i..].starts_with(from.as_str()) {
                    out.extend_from_slice(to.as_bytes());
                    i += from.len();
                    continue 'outer;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).expect("ASCII-only rewrites keep UTF-8 valid")
}

/// The whole shim: MSL (or GLSL-flavoured, or already-HLSL) fragment source in,
/// an HLSL body whose entry is `float4 fmain(GVOut gpIn) : SV_Target` out,
/// with the `gpModGl` prelude prepended.
pub fn to_hlsl_body(src: &str) -> String {
    let body = translate_entry(&translate_dialect(src));
    format!("{DIALECT_PRELUDE}\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Galaxigans' entry point, verbatim from world/49_galaxigans.mst:1770 —
    /// the exact input the sister port's shim was written against.
    #[test]
    fn galaxigans_entry() {
        let src = "fragment float4 fmain(VOut in [[stage_in]], constant Uniforms& u [[buffer(0)]]) {\n\
                   \x20   float t = u.time;\n\
                   \x20   int sc = int(u.p[0] + 0.5);\n\
                   \x20   float2 uv = in.uv;\n\
                   \x20   float a = u.aspect;\n\
                   \x20   return float4(uv, t, a);\n}";
        let out = translate_entry(src);
        assert!(out.starts_with("float4 fmain(GVOut gpIn) : SV_Target {"));
        assert!(out.contains("float t = time;"));
        assert!(out.contains("int sc = int(p[0] + 0.5);"));
        assert!(out.contains("float2 uv = gpIn.uv;"));
        assert!(out.contains("float a = aspect;"));
        assert!(!out.contains("[["));
    }

    #[test]
    fn dialect_calls_only() {
        // Calls are rewritten...
        assert_eq!(
            translate_dialect("fract(sin(dot(p, float2(127.1, 311.7))))"),
            "frac(sin(dot(p, float2(127.1, 311.7))))"
        );
        assert_eq!(translate_dialect("mix(a, b, t)"), "lerp(a, b, t)");
        assert_eq!(translate_dialect("mod(x, 4.0)"), "gpModGl(x, 4.0)");
        // ...variables of the same name are not.
        assert_eq!(translate_dialect("float mix = 3.0;"), "float mix = 3.0;");
        // ...and neither is a longer identifier containing one.
        assert_eq!(translate_dialect("refract(a, b, c)"), "refract(a, b, c)");
    }

    #[test]
    fn atan_arity() {
        assert_eq!(translate_dialect("atan(c.y, c.x)"), "atan2(c.y, c.x)");
        assert_eq!(translate_dialect("atan(x)"), "atan(x)");
        // Nested calls inside the arguments don't fool the counter.
        assert_eq!(
            translate_dialect("atan(min(a, b), c)"),
            "atan2(min(a, b), c)"
        );
    }

    #[test]
    fn hlsl_passes_through() {
        // Already-HLSL input is a fixed point of the whole shim: atan2 stays,
        // frac stays, and a body with no `fragment` keeps its signature.
        let src = "float4 fmain(GVOut gpIn) : SV_Target {\n\
                   \x20   return float4(frac(atan2(gpIn.uv.y, gpIn.uv.x)), 0, 0, 1);\n}";
        assert_eq!(translate_entry(&translate_dialect(src)), src);
    }

    #[test]
    fn discard_and_derivatives() {
        assert_eq!(translate_dialect("discard_fragment();"), "discard();");
        assert_eq!(translate_dialect("dfdx(v) + dfdy(v)"), "ddx(v) + ddy(v)");
        assert_eq!(translate_dialect("inversesqrt(d)"), "rsqrt(d)");
    }
}
