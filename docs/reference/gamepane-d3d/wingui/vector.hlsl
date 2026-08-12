// GPU anti-aliased vector primitives + glyph rendering for RGBA surfaces.
//
// Single instanced draw: 6 vertices per instance forming a screen-aligned quad
// covering the primitive's pixel-space bounding box (padded by 1px for AA).
// The pixel shader evaluates a signed distance function for the requested
// shape and converts it to alpha coverage using fwidth().
//
// Coordinate convention: pixel space, origin top-left. The vertex shader
// converts to NDC using the surface dimensions supplied in the cbuffer.
//
// Shape encoding (instance.shape_type):
//   0  RECT_FILLED      param0 = (corner_radius, _, _, _)
//   1  RECT_STROKED     param0 = (corner_radius, half_stroke, _, _)
//   2  LINE             param0 = (p0x, p0y, p1x, p1y)         param1.x = half_thickness
//   3  CIRCLE_FILLED    param0 = (cx, cy, radius, _)
//   4  CIRCLE_STROKED   param0 = (cx, cy, radius, half_stroke)
//   5  ARC              param0 = (cx, cy, radius, half_stroke) param1 = (rotation, half_aperture, _, _)
//   6  GLYPH            param0 = (uv_min_x, uv_min_y, uv_max_x, uv_max_y)

cbuffer VectorUniforms : register(b0) {
    float surface_width;
    float surface_height;
    float atlas_width;
    float atlas_height;
};

struct VectorInstance {
    float4 bounds   : TEXCOORD0;  // min_x, min_y, max_x, max_y (pixels)
    float4 param0   : TEXCOORD1;
    float4 param1   : TEXCOORD2;
    float4 color    : TEXCOORD3;
    uint   shape_id : TEXCOORD4;
};

struct VectorPSInput {
    float4 pos      : SV_POSITION;
    float2 px       : TEXCOORD0;   // pixel-space position
    nointerpolation float4 bounds : TEXCOORD1;
    nointerpolation float4 p0     : TEXCOORD2;
    nointerpolation float4 p1     : TEXCOORD3;
    nointerpolation float4 col    : TEXCOORD4;
    nointerpolation uint   shape  : TEXCOORD5;
};

VectorPSInput vector_vertex(uint vid : SV_VertexID, VectorInstance inst) {
    static const float2 quad[6] = {
        float2(0,0), float2(1,0), float2(0,1),
        float2(0,1), float2(1,0), float2(1,1)
    };
    const float2 t = quad[vid];

    // Pad bounds by 1px on every side so SDF AA at the edge isn't clipped.
    const float2 mn = inst.bounds.xy - 1.0;
    const float2 mx = inst.bounds.zw + 1.0;
    const float2 px = lerp(mn, mx, t);

    VectorPSInput o;
    o.pos = float4(
        (px.x / max(surface_width, 1.0)) * 2.0 - 1.0,
        1.0 - (px.y / max(surface_height, 1.0)) * 2.0,
        0.0, 1.0);
    o.px     = px;
    o.bounds = inst.bounds;
    o.p0     = inst.param0;
    o.p1     = inst.param1;
    o.col    = inst.color;
    o.shape  = inst.shape_id;
    return o;
}

Texture2D    glyph_atlas   : register(t0);
SamplerState glyph_sampler : register(s0);

float sd_round_box(float2 p, float2 half_size, float r) {
    float2 q = abs(p) - half_size + r;
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r;
}

float sd_segment(float2 p, float2 a, float2 b) {
    float2 pa = p - a;
    float2 ba = b - a;
    float h = saturate(dot(pa, ba) / max(dot(ba, ba), 1e-6));
    return length(pa - ba * h);
}

// Iñigo Quilez arc SDF. sc = (sin(half_aperture), cos(half_aperture)).
// p is in arc-local space (already rotated/translated).
float sd_arc(float2 p, float2 sc, float ra) {
    p.x = abs(p.x);
    return (sc.y * p.x > sc.x * p.y)
        ? length(p - sc * ra)
        : abs(length(p) - ra);
}

float coverage_from_sdf(float sdf) {
    float w = max(fwidth(sdf), 1e-6);
    return saturate(0.5 - sdf / w);
}

float4 vector_fragment(VectorPSInput input) : SV_TARGET {
    float coverage = 0.0;

    if (input.shape == 0) {
        // RECT_FILLED
        float2 center = 0.5 * (input.bounds.xy + input.bounds.zw);
        float2 half_size = 0.5 * (input.bounds.zw - input.bounds.xy);
        float r = min(input.p0.x, min(half_size.x, half_size.y));
        float sdf = sd_round_box(input.px - center, half_size, r);
        coverage = coverage_from_sdf(sdf);
    } else if (input.shape == 1) {
        // RECT_STROKED
        float2 center = 0.5 * (input.bounds.xy + input.bounds.zw);
        float2 half_size = 0.5 * (input.bounds.zw - input.bounds.xy);
        float r = min(input.p0.x, min(half_size.x, half_size.y));
        float half_stroke = max(input.p0.y, 0.5);
        float sdf = abs(sd_round_box(input.px - center, half_size - half_stroke, max(r - half_stroke, 0.0))) - half_stroke;
        coverage = coverage_from_sdf(sdf);
    } else if (input.shape == 2) {
        // LINE (capsule)
        float sdf = sd_segment(input.px, input.p0.xy, input.p0.zw) - max(input.p1.x, 0.5);
        coverage = coverage_from_sdf(sdf);
    } else if (input.shape == 3) {
        // CIRCLE_FILLED
        float sdf = length(input.px - input.p0.xy) - input.p0.z;
        coverage = coverage_from_sdf(sdf);
    } else if (input.shape == 4) {
        // CIRCLE_STROKED
        float sdf = abs(length(input.px - input.p0.xy) - input.p0.z) - max(input.p0.w, 0.5);
        coverage = coverage_from_sdf(sdf);
    } else if (input.shape == 5) {
        // ARC
        float2 c = input.p0.xy;
        float ra = input.p0.z;
        float half_stroke = max(input.p0.w, 0.5);
        float rot = input.p1.x;
        float half_aperture = input.p1.y;
        float2 d = input.px - c;
        float cs = cos(rot);
        float sn = sin(rot);
        // rotate so arc bisector points along +Y (matches sd_arc convention)
        float2 local = float2(d.x * cs + d.y * sn, -d.x * sn + d.y * cs);
        float2 sc = float2(sin(half_aperture), cos(half_aperture));
        float sdf = sd_arc(local, sc, ra) - half_stroke;
        coverage = coverage_from_sdf(sdf);
    } else if (input.shape == 6) {
        // GLYPH
        float2 size = max(input.bounds.zw - input.bounds.xy, float2(1, 1));
        float2 t = (input.px - input.bounds.xy) / size;
        if (t.x < 0.0 || t.x > 1.0 || t.y < 0.0 || t.y > 1.0) discard;
        float2 uv = lerp(input.p0.xy, input.p0.zw, t);
        float4 sample = glyph_atlas.Sample(glyph_sampler, uv);
        // Treat atlas alpha (or red, for single-channel atlases) as coverage.
        coverage = sample.a > 0.0 ? sample.a : sample.r;
    }

    if (coverage <= 0.0) discard;
    return float4(input.col.rgb, input.col.a * coverage);
}
