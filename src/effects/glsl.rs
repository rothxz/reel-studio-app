//! GLSL (450, Vulkan flavour) for each kind of pass, for the portable
//! renderer (`portable.rs`, which hands it to naga through wgpu). The same
//! programs as `msl.rs`; the dialect is GLSL-flavoured already, so only
//! three rewrites are needed: `MOD(` → `mod(` (GLSL's is floored),
//! `ATAN2(` → `atan(` (its two-argument form) and `S(` → `fx_S(`, a sample
//! of the pass's input.
//!
//! Every program is the one vertex stage (`VERTEX`: a quad over a rect of
//! the target in 0–1 units, y down) and a fragment stage reading texture 0
//! (and 1) at `fx_uv`. Bindings, all in set 0: 0 the rect, 1 the fragment
//! uniforms, 2 and 3 the textures, 4 the sampler. Textures are read at
//! level 0 explicitly (no mipmaps; no derivatives, so a read may sit in
//! any branch).

use super::package::EffectKind;

pub const VERTEX: &str = r#"#version 450
layout(set = 0, binding = 0) uniform FxRect { vec4 rect; } fx_rect;
layout(location = 0) out vec2 fx_uv;

void main() {
    vec2 corner = vec2(float(gl_VertexIndex & 1), float(gl_VertexIndex >> 1));
    vec2 at = mix(fx_rect.rect.xy, fx_rect.rect.zw, corner);
    fx_uv = corner;
    gl_Position = vec4(at.x * 2.0 - 1.0, 1.0 - at.y * 2.0, 0.0, 1.0);
}
"#;

const COMMON: &str = r#"#version 450
layout(location = 0) in vec2 fx_uv;
layout(location = 0) out vec4 fx_out;
layout(set = 0, binding = 2) uniform texture2D fx_tex0;
layout(set = 0, binding = 3) uniform texture2D fx_tex1;
layout(set = 0, binding = 4) uniform sampler fx_smp;

vec4 fx_read0(vec2 uv) { return textureLod(sampler2D(fx_tex0, fx_smp), uv, 0.0); }
vec4 fx_read1(vec2 uv) { return textureLod(sampler2D(fx_tex1, fx_smp), uv, 0.0); }

vec4 fx_straight(vec4 c) {
    return c.a > 0.0 ? vec4(clamp(c.rgb / c.a, 0.0, 1.0), c.a) : vec4(0.0);
}
"#;

/// The look passes' uniforms (`Uniforms` in draw.rs).
const FX_UNIFORMS: &str = r#"
layout(set = 0, binding = 1) uniform FxUniforms {
    vec4 row0;
    vec4 row1;
    vec4 row2;
    vec4 row3;
    vec4 offset;
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
} fx_u;
"#;

/// Helpers effect bodies may call.
const EFFECT_HELPERS: &str = r#"
// Cheap deterministic pseudo-random value in [0, 1) per point.
float fx_hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123);
}

// Smooth value noise: the hashes of the four lattice corners, eased.
float fx_noise(vec2 p) {
    vec2 cell = floor(p);
    vec2 f = fract(p);
    vec2 w = f * f * (3.0 - 2.0 * f);
    float bottom = mix(fx_hash(cell), fx_hash(cell + vec2(1.0, 0.0)), w.x);
    float top = mix(fx_hash(cell + vec2(0.0, 1.0)), fx_hash(cell + vec2(1.0, 1.0)), w.x);
    return mix(bottom, top, w.y);
}

// Four octaves of noise, each finer and half as strong.
float fx_fbm(vec2 p) {
    float sum = 0.0;
    float amplitude = 0.5;
    for (int octave = 0; octave < 4; octave++) {
        sum += amplitude * fx_noise(p);
        p = p * 2.03 + vec2(17.1, 9.3);
        amplitude *= 0.5;
    }
    return sum;
}

// Perceived brightness (Rec. 601 weights).
float fx_luma(vec3 c) { return dot(c, vec3(0.299, 0.587, 0.114)); }

// A fully saturated colour for hue h (0-1 around the wheel).
vec3 fx_hue(float h) {
    vec3 phase = fract(h + vec3(0.0, 0.6667, 0.3333));
    return clamp(abs(phase * 6.0 - 3.0) - 1.0, 0.0, 1.0);
}

// p turned by a radians (counter-clockwise in a y-up frame).
vec2 fx_rot(vec2 p, float a) {
    float c = cos(a);
    float s = sin(a);
    return vec2(c * p.x - s * p.y, s * p.x + c * p.y);
}

float fx_A(vec2 uv) { return fx_read0(clamp(uv, 0.0, 1.0)).a; }
vec3 fx_S(vec2 uv) {
    return fx_straight(fx_read0(clamp(uv, 0.0, 1.0))).rgb;
}
"#;

/// Helpers filter bodies may call: the building blocks of a grade.
const FILTER_HELPERS: &str = r#"
// Luminance (Rec. 709 weights).
float fl_luma(vec3 c) { return dot(c, vec3(0.2126, 0.7152, 0.0722)); }

// Saturation: 0 grey, 1 unchanged, above 1 richer.
vec3 fl_sat(vec3 c, float s) { return mix(vec3(fl_luma(c)), c, s); }

// Vibrance: saturation that spares colours already saturated.
vec3 fl_vib(vec3 c, float v) {
    float chroma = clamp(max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b)), 0.0, 1.0);
    return fl_sat(c, 1.0 + v * (1.0 - chroma));
}

// Contrast around mid grey.
vec3 fl_contrast(vec3 c, float k) { return (c - 0.5) * k + 0.5; }

// White balance: temperature warms (red up, blue down), tint pulls green.
vec3 fl_wb(vec3 c, float temp, float tint) {
    return c * vec3(1.0 + 0.1 * temp, 1.0 - 0.08 * tint, 1.0 - 0.1 * temp);
}

// Hue shift by a radians, turning the chroma plane of YIQ.
vec3 fl_hue(vec3 c, float a) {
    float y = dot(c, vec3(0.299, 0.587, 0.114));
    float i = dot(c, vec3(0.596, -0.274, -0.322));
    float q = dot(c, vec3(0.211, -0.523, 0.312));
    float cs = cos(a);
    float sn = sin(a);
    float i2 = i * cs - q * sn;
    float q2 = i * sn + q * cs;
    return vec3(y + 0.956 * i2 + 0.621 * q2,
                y - 0.272 * i2 - 0.647 * q2,
                y - 1.106 * i2 + 1.703 * q2);
}

// Catmull-Rom between p1 and p2 (t 0-1), p0 / p3 its neighbours.
float fl_cr(float p0, float p1, float p2, float p3, float t) {
    float t2 = t * t;
    float t3 = t2 * t;
    return 0.5 * (2.0 * p1
                  + (p2 - p0) * t
                  + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                  + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3);
}

// A tone curve through five points at x = 0, .25, .5, .75, 1, smooth
// between them, its ends continued straight.
float fl_curve(float x, float y0, float y1, float y2, float y3, float y4) {
    float s = clamp(x, 0.0, 1.0) * 4.0;
    if (s < 1.0) return fl_cr(2.0 * y0 - y1, y0, y1, y2, s);
    if (s < 2.0) return fl_cr(y0, y1, y2, y3, s - 1.0);
    if (s < 3.0) return fl_cr(y1, y2, y3, y4, s - 2.0);
    return fl_cr(y2, y3, y4, 2.0 * y4 - y3, s - 3.0);
}

vec3 fl_curve3(vec3 c, float y0, float y1, float y2, float y3, float y4) {
    return vec3(fl_curve(c.r, y0, y1, y2, y3, y4),
                fl_curve(c.g, y0, y1, y2, y3, y4),
                fl_curve(c.b, y0, y1, y2, y3, y4));
}

// Split toning: the shadows take sh's hue, the highlights hi's, by amt;
// bal moves the split. Only the tones' chroma is added (no brightening).
vec3 fl_split(vec3 c, vec3 sh, vec3 hi, float amt, float bal) {
    float l = fl_luma(c);
    float shadows = 1.0 - smoothstep(0.0, 0.6 + bal, l);
    float highlights = smoothstep(0.4 + bal, 1.0, l);
    vec3 shTone = sh - fl_luma(sh);
    vec3 hiTone = hi - fl_luma(hi);
    return c + (shTone * shadows + hiTone * highlights) * (amt * 1.6);
}

// Bleach bypass: the picture overlaid with its own luminance.
vec3 fl_bleach(vec3 c, float amt) {
    vec3 l = vec3(fl_luma(c));
    vec3 low = 2.0 * c * l;
    vec3 high = 1.0 - 2.0 * (1.0 - c) * (1.0 - l);
    return mix(c, mix(low, high, step(vec3(0.5), l)), amt);
}

// Duotone: luminance mapped from dark to light.
vec3 fl_duo(vec3 c, vec3 dark, vec3 light) {
    return mix(dark, light, clamp(fl_luma(c), 0.0, 1.0));
}

// Lifted blacks (a matte look).
vec3 fl_fade(vec3 c, float f) { return c * (1.0 - f) + f; }

// Darkened corners, round whatever the picture's shape.
vec3 fl_vignette(vec3 c, vec2 uv, float ar, float amt) {
    vec2 d = (uv - 0.5) * vec2(min(ar, 1.0), min(1.0 / ar, 1.0)) * 2.0;
    return c * (1.0 - amt * smoothstep(0.45, 1.35, length(d)));
}
"#;

/// Rewrites a dialect body as GLSL (see the module comment). `sample`
/// says whether `S(` is a sample (a filter has none).
pub fn translate(body: &str, sample: bool) -> String {
    let mut out = String::with_capacity(body.len() + 16);
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
            "MOD" if call => out.push_str("mod"),
            "ATAN2" if call => out.push_str("atan"),
            "S" if call && sample => out.push_str("fx_S"),
            "A" if call && sample => out.push_str("fx_A"),
            _ => out.push_str(&word),
        }
    }
    out
}

/// A pass that copies its input (a picture fitted onto the canvas).
pub fn copy() -> String {
    format!("{COMMON}\nvoid main() {{ fx_out = fx_read0(fx_uv); }}\n")
}

/// A 4×5 colour matrix on straight colour (a Classic filter).
pub fn matrix() -> String {
    format!(
        "{COMMON}{FX_UNIFORMS}
void main() {{
    vec4 c = fx_straight(fx_read0(fx_uv));
    vec4 m = vec4(dot(fx_u.row0, c), dot(fx_u.row1, c), dot(fx_u.row2, c), dot(fx_u.row3, c))
        + fx_u.offset;
    m = clamp(m, 0.0, 1.0);
    fx_out = vec4(m.rgb * m.a, m.a);
}}
"
    )
}

/// A packaged filter: `fl_filter`, mixed into the colour by `k`.
pub fn filter(body: &str) -> String {
    format!(
        "{COMMON}{FX_UNIFORMS}{FILTER_HELPERS}
vec3 fl_filter(vec3 c, vec2 uv, float ar) {{
{}
}}

void main() {{
    vec4 c = fx_straight(fx_read0(fx_uv));
    vec3 graded = clamp(fl_filter(c.rgb, fx_uv, fx_u.ar), 0.0, 1.0);
    fx_out = vec4(mix(c.rgb, graded, fx_u.k) * c.a, c.a);
}}
",
        translate(body, false)
    )
}

/// A video effect: a distortion replaces the colour; a blend effect's
/// colour is combined with the picture under it.
pub fn effect(kind: EffectKind, body: &str) -> String {
    let combine = match kind {
        EffectKind::Distortion => "vec3 rgb = c;",
        // Added light, capped at white.
        EffectKind::Plus => "vec3 rgb = min(under.rgb + c, 1.0);",
        EffectKind::Screen => "vec3 rgb = c + under.rgb - c * under.rgb;",
        EffectKind::Multiply => "vec3 rgb = c * under.rgb;",
    };
    format!(
        "{COMMON}{FX_UNIFORMS}{EFFECT_HELPERS}
vec3 fx_effect(vec2 uv, float time, float k, float ar, float pr, float dur,
               float p1, float p2, float p3, float p4) {{
{}
}}

void main() {{
    vec4 under = fx_straight(fx_read0(fx_uv));
    vec3 c = fx_effect(fx_uv, fx_u.time, fx_u.k, fx_u.ar, fx_u.pr, fx_u.dur,
                       fx_u.p1, fx_u.p2, fx_u.p3, fx_u.p4);
    {combine}
    rgb = clamp(rgb, 0.0, 1.0);
    fx_out = vec4(rgb * under.a, under.a);
}}
",
        translate(body, true)
    )
}

/// A transition's reveal of the incoming picture: the body returns its
/// premultiplied colour at `uv` (`S(` its straight colour, `A(` its
/// alpha), `pr` 0–1 through the window, `dur` the window in seconds.
pub fn reveal(body: &str) -> String {
    format!(
        "{COMMON}{FX_UNIFORMS}{EFFECT_HELPERS}
vec4 an_body(vec2 uv, float time, float ar, float pr, float dur) {{
{}
}}

void main() {{
    vec4 c = an_body(fx_uv, fx_u.time, fx_u.ar, fx_u.pr, fx_u.dur);
    float a = clamp(c.a, 0.0, 1.0);
    fx_out = vec4(min(max(c.rgb, vec3(0.0)), vec3(a)), a);
}}
",
        translate(body, true)
    )
}

/// A one-direction Gaussian blur (run across, then down): `dir` the uv
/// step of one pixel, `sigma.x` the sigma in pixels.
pub fn blur() -> String {
    format!(
        "{COMMON}
layout(set = 0, binding = 1) uniform BlurUniforms {{
    vec4 dir;
    vec4 sigma;
}} fx_b;

void main() {{
    float sigma = max(fx_b.sigma.x, 0.5);
    // 24 taps a side spread over 3 sigma (linear sampling fills between).
    float stride = max(sigma * 3.0 / 24.0, 1.0);
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int i = -24; i <= 24; i++) {{
        float d = float(i) * stride;
        float w = exp(-0.5 * (d * d) / (sigma * sigma));
        sum += fx_read0(fx_uv + fx_b.dir.xy * d) * w;
        total += w;
    }}
    fx_out = sum / total;
}}
"
    )
}

/// Places one picture on the canvas over what's below it, as `msl::compose`
/// does (see there): texture 0 the canvas so far, texture 1 the picture.
pub fn compose() -> String {
    format!("{COMMON}{COMPOSE}")
}

const COMPOSE: &str = r#"
layout(set = 0, binding = 1) uniform ComposeUniforms {
    vec4 canvas;   // width, height (px), frame aspect (w / h), blend mode
    vec4 place;    // centre x, y (px), shown width, height (px)
    vec4 turn;     // cos, sin of the clockwise turn, opacity, flags
    vec4 crop;     // x, y, width, height in the source, 0..1
    vec4 mask0;    // centre x, y, width, height (frame units)
    vec4 mask1;    // cos, sin of the mask's turn, feather, shape
    vec4 straight; // cos, sin of the crop's turn undone, its cover scale, source aspect
} cu;

// Flags in turn.w.
const int MIRROR_X = 1;
const int MIRROR_Y = 2;
const int HAS_MASK = 4;
const int INVERT = 8;

// Signed distance to a heart (lobes up, tip at the origin, about 1 tall).
float cm_heart(vec2 p) {
    p.x = abs(p.x);
    if (p.x + p.y > 1.0) {
        return length(p - vec2(0.25, 0.75)) - sqrt(2.0) / 4.0;
    }
    vec2 a = p - vec2(0.0, 1.0);
    float t = 0.5 * max(p.x + p.y, 0.0);
    vec2 b = p - vec2(t);
    return sqrt(min(dot(a, a), dot(b, b))) * sign(p.x - p.y);
}

// Signed distance to a five-point star of outer radius 1, a tip up:
// folded into one tenth of the star by its mirror lines, then the
// distance to the edge from the tip to the inner corner.
float cm_star(vec2 p) {
    vec2 k = vec2(0.809016994375, -0.587785252292);  // cos 36, -sin 36
    p.x = abs(p.x);
    p -= 2.0 * max(dot(k, p), 0.0) * k;
    p -= 2.0 * max(dot(vec2(-k.x, k.y), p), 0.0) * vec2(-k.x, k.y);
    p.x = abs(p.x);
    p.y -= 1.0;
    float inner = 0.46;
    vec2 edge = vec2(-inner * k.y, inner * k.x - 1.0);
    float h = clamp(dot(p, edge) / dot(edge, edge), 0.0, 1.0);
    return length(p - edge * h) * sign(p.y * edge.x - p.x * edge.y);
}

// 0 (hidden) .. 1 (shown) at frame point `uv` (0..1, y down).
float cm_mask(vec2 uv) {
    float aspect = cu.canvas.z;
    vec2 d = vec2((uv.x - cu.mask0.x) * aspect, uv.y - cu.mask0.y);
    float c = cu.mask1.x;
    float s = cu.mask1.y;
    vec2 p = vec2(c * d.x + s * d.y, -s * d.x + c * d.y);
    float hx = max(cu.mask0.z * 0.5 * aspect, 1e-4);
    float hy = max(cu.mask0.w * 0.5, 1e-4);
    int shape = int(cu.mask1.w + 0.5);
    float sd;
    if (shape == 0) {
        // Horizontal: the half above the line.
        sd = p.y;
    } else if (shape == 1) {
        // Mirror: a band `height` thick.
        sd = abs(p.y) - hy;
    } else if (shape == 2) {
        sd = (length(p / vec2(hx, hy)) - 1.0) * min(hx, hy);
    } else if (shape == 3) {
        vec2 q = abs(p) - vec2(hx, hy);
        sd = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
    } else if (shape == 4) {
        // The heart's box: half-width 0.6036, height 1.1036 in its units.
        vec2 q = vec2(p.x / hx, -p.y / hy);
        sd = cm_heart(vec2(q.x * 0.6036, (q.y * 0.5 + 0.5) * 1.1036)) * min(hx, hy) / 0.58;
    } else {
        // The star's box: half-width sin 72, half-height (1 + cos 36) / 2.
        vec2 q = vec2(p.x / hx * 0.9511, -p.y / hy * 0.9045 + 0.0955);
        sd = cm_star(q) * min(hx, hy) / 0.9278;
    }
    float soft = shape == 0 ? 0.5 : min(hx, hy);
    float w = max(cu.mask1.z * soft, 0.0015);
    float t = clamp(0.5 - sd / (2.0 * w), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

float cm_soft(float b, float s) {
    if (s <= 0.5) return b - (1.0 - 2.0 * s) * b * (1.0 - b);
    float d = b <= 0.25 ? ((16.0 * b - 12.0) * b + 4.0) * b : sqrt(b);
    return b + (2.0 * s - 1.0) * (d - b);
}

float cm_burn(float b, float s) {
    if (b >= 1.0) return 1.0;
    if (s <= 0.0) return 0.0;
    return 1.0 - min(1.0, (1.0 - b) / s);
}

float cm_dodge(float b, float s) {
    if (b <= 0.0) return 0.0;
    if (s >= 1.0) return 1.0;
    return min(1.0, b / (1.0 - s));
}

// Hard light: multiply where the picture is dark, screen where it's light.
float cm_hard1(float b, float s) {
    float s2 = 2.0 * s;
    return s <= 0.5 ? b * s2 : b + (s2 - 1.0) - b * (s2 - 1.0);
}

vec3 cm_hard(vec3 b, vec3 s) {
    return vec3(cm_hard1(b.r, s.r), cm_hard1(b.g, s.g), cm_hard1(b.b, s.b));
}

// Backdrop `b`, picture `s` (straight colour) -> the mode's colour.
vec3 cm_blend(int mode, vec3 b, vec3 s) {
    if (mode == 1) return min(b, s);                          // darken
    if (mode == 2) return max(b, s);                          // lighten
    if (mode == 3) return b + s - b * s;                      // filter (screen)
    if (mode == 4) return cm_hard(s, b);                      // overlay
    if (mode == 5) return cm_hard(b, s);                      // hard light
    if (mode == 6) return vec3(cm_soft(b.r, s.r), cm_soft(b.g, s.g), cm_soft(b.b, s.b));
    if (mode == 7) return b * s;                              // burn (multiply)
    if (mode == 8) return vec3(cm_burn(b.r, s.r), cm_burn(b.g, s.g), cm_burn(b.b, s.b));
    if (mode == 9) return vec3(cm_dodge(b.r, s.r), cm_dodge(b.g, s.g), cm_dodge(b.b, s.b));
    if (mode == 10) return b + s - 2.0 * b * s;               // slide (exclusion)
    return s;
}

void main() {
    vec4 under = fx_read0(fx_uv);
    // Canvas pixel -> the frame, undoing the move and the clockwise turn.
    vec2 p = fx_uv * cu.canvas.xy - cu.place.xy;
    float c = cu.turn.x;
    float s = cu.turn.y;
    vec2 q = vec2(c * p.x + s * p.y, -s * p.x + c * p.y);
    vec2 f = q / cu.place.zw + 0.5;
    // Half a pixel of soft edge, so a turned frame isn't jagged.
    vec2 inset = (0.5 - abs(f - 0.5)) * cu.place.zw;
    float inside = clamp(min(inset.x, inset.y) + 0.5, 0.0, 1.0);
    if (inside <= 0.0) {
        fx_out = under;
        return;
    }
    int flags = int(cu.turn.w + 0.5);
    if ((flags & MIRROR_X) != 0) f.x = 1.0 - f.x;
    if ((flags & MIRROR_Y) != 0) f.y = 1.0 - f.y;
    f = clamp(f, 0.0, 1.0);
    // Into the source: the crop box, then the straighten (the source turned
    // about the box's centre and grown by the cover scale), in source pixels
    // shaped by its aspect.
    vec2 shape = vec2(cu.straight.w, 1.0);
    vec2 centre = (cu.crop.xy + cu.crop.zw * 0.5) * shape;
    vec2 d = (cu.crop.xy + f * cu.crop.zw) * shape - centre;
    vec2 v = vec2(d.x * cu.straight.x - d.y * cu.straight.y,
                  d.x * cu.straight.y + d.y * cu.straight.x) / cu.straight.z;
    vec2 src = clamp((centre + v) / shape, 0.0, 1.0);
    vec4 pic = fx_straight(fx_read1(src));
    float cover = 1.0;
    if ((flags & HAS_MASK) != 0) {
        cover = cm_mask(f);
        if ((flags & INVERT) != 0) cover = 1.0 - cover;
    }
    float a = pic.a * cover * cu.turn.z * inside;
    vec4 back = fx_straight(under);
    vec3 mixed = cm_blend(int(cu.canvas.w + 0.5), back.rgb, pic.rgb);
    // The canvas below is opaque (it starts as a solid colour).
    fx_out = vec4(mix(back.rgb, clamp(mixed, 0.0, 1.0), a), 1.0);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_the_dialect_only_where_it_should() {
        let body = "vec2 o = vec2(1.0e2, 2.0);\nvec3 S2 = S(uv + o) * MOD(time, 2.0);\nfloat a = ATAN2(o.y, o.x); vec4 x = myvec3(SS(1));";
        assert_eq!(
            translate(body, true),
            "vec2 o = vec2(1.0e2, 2.0);\nvec3 S2 = fx_S(uv + o) * mod(time, 2.0);\nfloat a = atan(o.y, o.x); vec4 x = myvec3(SS(1));"
        );
        // A filter has no S().
        assert_eq!(translate("S(1)", false), "S(1)");
    }
}
