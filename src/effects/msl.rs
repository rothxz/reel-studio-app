//! Metal shader source for each kind of pass, with a package's body spliced
//! in. The dialect is GLSL-flavoured; four rewrites make it Metal: `vecN` →
//! `floatN`, `MOD(` → `fx_mod(` (GLSL's floored modulo), `ATAN2(` →
//! `atan2(`, and `S(` → a sample of the pass's input. Bodies call the
//! `fx_` (effects) and `fl_` (filters) helpers below, which this app
//! defines.
//!
//! Every program is one full-quad vertex stage (`fx_vertex`, drawn over a
//! rect of the target given in 0–1 units, y down) and one fragment stage
//! (`fx_fragment`) reading the input texture at the quad's uv, with the
//! uniforms of `Uniforms` (src/effects/metal.rs).

use super::package::EffectKind;

const COMMON: &str = r#"#include <metal_stdlib>
using namespace metal;

struct FxVaryings {
    float4 position [[position]];
    float2 uv;
};

struct FxUniforms {
    float4 row0;
    float4 row1;
    float4 row2;
    float4 row3;
    float4 offset;
    float time;
    float k;
    float ar;
    float pr;
    float dur;
    float p1;
    float p2;
    float p3;
    float p4;
    float pad0;
    float pad1;
    float pad2;
};

vertex FxVaryings fx_vertex(uint vid [[vertex_id]], constant float4 &rect [[buffer(0)]]) {
    float2 corner = float2(float(vid & 1u), float(vid >> 1u));
    float2 at = mix(rect.xy, rect.zw, corner);
    FxVaryings out;
    out.position = float4(at.x * 2.0 - 1.0, 1.0 - at.y * 2.0, 0.0, 1.0);
    out.uv = corner;
    return out;
}

static float4 fx_straight(float4 c) {
    return c.a > 0.0 ? float4(clamp(c.rgb / c.a, 0.0, 1.0), c.a) : float4(0.0);
}
"#;

/// Helpers effect bodies may call.
const EFFECT_HELPERS: &str = r#"
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunused-function"
// GLSL's mod(): floored, so negative inputs wrap upwards.
static float fx_mod(float a, float b) { return a - b * floor(a / b); }
static float2 fx_mod(float2 a, float b) { return a - b * floor(a / b); }
static float3 fx_mod(float3 a, float b) { return a - b * floor(a / b); }
static float2 fx_mod(float2 a, float2 b) { return a - b * floor(a / b); }

// Cheap deterministic pseudo-random value in [0, 1) per point.
static float fx_hash(float2 p) {
    return fract(sin(dot(p, float2(127.1, 311.7))) * 43758.5453123);
}

// Smooth value noise: the hashes of the four lattice corners, eased.
static float fx_noise(float2 p) {
    float2 cell = floor(p);
    float2 f = fract(p);
    float2 w = f * f * (3.0 - 2.0 * f);
    float bottom = mix(fx_hash(cell), fx_hash(cell + float2(1.0, 0.0)), w.x);
    float top = mix(fx_hash(cell + float2(0.0, 1.0)), fx_hash(cell + float2(1.0, 1.0)), w.x);
    return mix(bottom, top, w.y);
}

// Four octaves of noise, each finer and half as strong.
static float fx_fbm(float2 p) {
    float sum = 0.0;
    float amplitude = 0.5;
    for (int octave = 0; octave < 4; octave++) {
        sum += amplitude * fx_noise(p);
        p = p * 2.03 + float2(17.1, 9.3);
        amplitude *= 0.5;
    }
    return sum;
}

// Perceived brightness (Rec. 601 weights).
static float fx_luma(float3 c) { return dot(c, float3(0.299, 0.587, 0.114)); }

// A fully saturated colour for hue h (0-1 around the wheel).
static float3 fx_hue(float h) {
    float3 phase = fract(h + float3(0.0, 0.6667, 0.3333));
    return clamp(abs(phase * 6.0 - 3.0) - 1.0, 0.0, 1.0);
}

// p turned by a radians (counter-clockwise in a y-up frame).
static float2 fx_rot(float2 p, float a) {
    float c = cos(a);
    float s = sin(a);
    return float2(c * p.x - s * p.y, s * p.x + c * p.y);
}

static float3 fx_S(texture2d<float> fx_tex, sampler fx_smp, float2 uv) {
    return fx_straight(fx_tex.sample(fx_smp, clamp(uv, 0.0, 1.0))).rgb;
}
#pragma clang diagnostic pop
"#;

/// Helpers filter bodies may call: the building blocks of a grade.
const FILTER_HELPERS: &str = r#"
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunused-function"
// Luminance (Rec. 709 weights).
static float fl_luma(float3 c) { return dot(c, float3(0.2126, 0.7152, 0.0722)); }

// Saturation: 0 grey, 1 unchanged, above 1 richer.
static float3 fl_sat(float3 c, float s) { return mix(float3(fl_luma(c)), c, s); }

// Vibrance: saturation that spares colours already saturated.
static float3 fl_vib(float3 c, float v) {
    float chroma = clamp(max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b)), 0.0, 1.0);
    return fl_sat(c, 1.0 + v * (1.0 - chroma));
}

// Contrast around mid grey.
static float3 fl_contrast(float3 c, float k) { return (c - 0.5) * k + 0.5; }

// White balance: temperature warms (red up, blue down), tint pulls green.
static float3 fl_wb(float3 c, float temp, float tint) {
    return c * float3(1.0 + 0.1 * temp, 1.0 - 0.08 * tint, 1.0 - 0.1 * temp);
}

// Hue shift by a radians, turning the chroma plane of YIQ.
static float3 fl_hue(float3 c, float a) {
    float y = dot(c, float3(0.299, 0.587, 0.114));
    float i = dot(c, float3(0.596, -0.274, -0.322));
    float q = dot(c, float3(0.211, -0.523, 0.312));
    float cs = cos(a);
    float sn = sin(a);
    float i2 = i * cs - q * sn;
    float q2 = i * sn + q * cs;
    return float3(y + 0.956 * i2 + 0.621 * q2,
                  y - 0.272 * i2 - 0.647 * q2,
                  y - 1.106 * i2 + 1.703 * q2);
}

// Catmull-Rom between p1 and p2 (t 0-1), p0 / p3 its neighbours.
static float fl_cr(float p0, float p1, float p2, float p3, float t) {
    float t2 = t * t;
    float t3 = t2 * t;
    return 0.5 * (2.0 * p1
                  + (p2 - p0) * t
                  + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                  + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3);
}

// A tone curve through five points at x = 0, .25, .5, .75, 1, smooth
// between them, its ends continued straight.
static float fl_curve(float x, float y0, float y1, float y2, float y3, float y4) {
    float s = clamp(x, 0.0, 1.0) * 4.0;
    if (s < 1.0) return fl_cr(2.0 * y0 - y1, y0, y1, y2, s);
    if (s < 2.0) return fl_cr(y0, y1, y2, y3, s - 1.0);
    if (s < 3.0) return fl_cr(y1, y2, y3, y4, s - 2.0);
    return fl_cr(y2, y3, y4, 2.0 * y4 - y3, s - 3.0);
}

static float3 fl_curve3(float3 c, float y0, float y1, float y2, float y3, float y4) {
    return float3(fl_curve(c.r, y0, y1, y2, y3, y4),
                  fl_curve(c.g, y0, y1, y2, y3, y4),
                  fl_curve(c.b, y0, y1, y2, y3, y4));
}

// Split toning: the shadows take sh's hue, the highlights hi's, by amt;
// bal moves the split. Only the tones' chroma is added (no brightening).
static float3 fl_split(float3 c, float3 sh, float3 hi, float amt, float bal) {
    float l = fl_luma(c);
    float shadows = 1.0 - smoothstep(0.0, 0.6 + bal, l);
    float highlights = smoothstep(0.4 + bal, 1.0, l);
    float3 shTone = sh - fl_luma(sh);
    float3 hiTone = hi - fl_luma(hi);
    return c + (shTone * shadows + hiTone * highlights) * (amt * 1.6);
}

// Bleach bypass: the picture overlaid with its own luminance.
static float3 fl_bleach(float3 c, float amt) {
    float3 l = float3(fl_luma(c));
    float3 low = 2.0 * c * l;
    float3 high = 1.0 - 2.0 * (1.0 - c) * (1.0 - l);
    return mix(c, mix(low, high, step(float3(0.5), l)), amt);
}

// Duotone: luminance mapped from dark to light.
static float3 fl_duo(float3 c, float3 dark, float3 light) {
    return mix(dark, light, clamp(fl_luma(c), 0.0, 1.0));
}

// Lifted blacks (a matte look).
static float3 fl_fade(float3 c, float f) { return c * (1.0 - f) + f; }

// Darkened corners, round whatever the picture's shape.
static float3 fl_vignette(float3 c, float2 uv, float ar, float amt) {
    float2 d = (uv - 0.5) * float2(min(ar, 1.0), min(1.0 / ar, 1.0)) * 2.0;
    return c * (1.0 - amt * smoothstep(0.45, 1.35, length(d)));
}
#pragma clang diagnostic pop
"#;

/// Rewrites a dialect body as Metal (see the module comment). `sample`
/// is what `S(` becomes (None: a filter, which has no `S`).
pub fn translate(body: &str, sample: Option<&str>) -> String {
    let mut out = String::with_capacity(body.len() + 64);
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            // A number (2.0, 1e3): never an identifier.
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if !(c.is_ascii_alphabetic() || c == '_') {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        let call = chars.get(i) == Some(&'(');
        match word.as_str() {
            "vec2" => out.push_str("float2"),
            "vec3" => out.push_str("float3"),
            "vec4" => out.push_str("float4"),
            "MOD" if call => out.push_str("fx_mod"),
            "ATAN2" if call => out.push_str("atan2"),
            "S" if call && sample.is_some() => {
                out.push_str(sample.unwrap_or_default());
                i += 1; // the "(" is part of the replacement
            }
            _ => out.push_str(&word),
        }
    }
    out
}

/// A pass that copies its input (the clip fitted onto the canvas).
pub fn copy() -> String {
    format!(
        "{COMMON}
fragment float4 fx_fragment(FxVaryings in [[stage_in]], texture2d<float> src [[texture(0)]],
                            sampler smp [[sampler(0)]], constant FxUniforms &u [[buffer(0)]]) {{
    return src.sample(smp, in.uv);
}}
"
    )
}

/// A 4×5 colour matrix on straight colour (a Classic filter).
pub fn matrix() -> String {
    format!(
        "{COMMON}
fragment float4 fx_fragment(FxVaryings in [[stage_in]], texture2d<float> src [[texture(0)]],
                            sampler smp [[sampler(0)]], constant FxUniforms &u [[buffer(0)]]) {{
    float4 c = fx_straight(src.sample(smp, in.uv));
    float4 m = float4(dot(u.row0, c), dot(u.row1, c), dot(u.row2, c), dot(u.row3, c)) + u.offset;
    m = clamp(m, 0.0, 1.0);
    return float4(m.rgb * m.a, m.a);
}}
"
    )
}

/// A packaged filter: `fl_filter`, mixed into the colour by `u.k`.
pub fn filter(body: &str) -> String {
    format!(
        "{COMMON}{FILTER_HELPERS}
static float3 fl_filter(float3 c, float2 uv, float ar) {{
{}
}}

fragment float4 fx_fragment(FxVaryings in [[stage_in]], texture2d<float> src [[texture(0)]],
                            sampler smp [[sampler(0)]], constant FxUniforms &u [[buffer(0)]]) {{
    float4 c = fx_straight(src.sample(smp, in.uv));
    float3 graded = clamp(fl_filter(c.rgb, in.uv, u.ar), 0.0, 1.0);
    return float4(mix(c.rgb, graded, u.k) * c.a, c.a);
}}
",
        translate(body, None)
    )
}

/// A video effect: a distortion replaces the colour; a blend effect's
/// colour is combined with the picture under it.
pub fn effect(kind: EffectKind, body: &str) -> String {
    let combine = match kind {
        EffectKind::Distortion => "float3 rgb = c;",
        // Added light, capped at white.
        EffectKind::Plus => "float3 rgb = min(under.rgb + c, 1.0);",
        EffectKind::Screen => "float3 rgb = c + under.rgb - c * under.rgb;",
        EffectKind::Multiply => "float3 rgb = c * under.rgb;",
    };
    format!(
        "{COMMON}{EFFECT_HELPERS}
static float3 fx_effect(texture2d<float> fx_tex, sampler fx_smp, float2 uv, float time, float k,
                        float ar, float pr, float dur, float p1, float p2, float p3, float p4) {{
{}
}}

fragment float4 fx_fragment(FxVaryings in [[stage_in]], texture2d<float> src [[texture(0)]],
                            sampler smp [[sampler(0)]], constant FxUniforms &u [[buffer(0)]]) {{
    float4 under = fx_straight(src.sample(smp, in.uv));
    float3 c = fx_effect(src, smp, in.uv, u.time, u.k, u.ar, u.pr, u.dur, u.p1, u.p2, u.p3, u.p4);
    {combine}
    rgb = clamp(rgb, 0.0, 1.0);
    return float4(rgb * under.a, under.a);
}}
",
        translate(body, Some("fx_S(fx_tex, fx_smp, "))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_the_dialect_only_where_it_should() {
        let body = "vec2 o = vec2(1.0e2, 2.0);\nvec3 S2 = S(uv + o) * MOD(time, 2.0);\nfloat a = ATAN2(o.y, o.x); vec4 x = myvec3(SS(1));";
        assert_eq!(
            translate(body, Some("fx_S(t, s, ")),
            "float2 o = float2(1.0e2, 2.0);\nfloat3 S2 = fx_S(t, s, uv + o) * fx_mod(time, 2.0);\nfloat a = atan2(o.y, o.x); float4 x = myvec3(SS(1));"
        );
        // A filter has no S().
        assert_eq!(translate("S(1)", None), "S(1)");
    }
}
