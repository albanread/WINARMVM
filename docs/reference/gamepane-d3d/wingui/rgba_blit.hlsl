cbuffer RgbaBlit : register(b0) {
    // dst quad in NDC (precomputed by CPU from dst rect + dst surface size)
    float2 dst_pos_min;     // top-left in NDC (-1..1)
    float2 dst_pos_max;     // bottom-right in NDC (-1..1)
    // src rect in source-texture UV space [0,1]
    float2 src_uv_min;
    float2 src_uv_max;
    // tint_rgba is multiplied with the sample. 1,1,1,1 = passthrough.
    float4 tint_rgba;
    uint   blit_pad0;
    uint   blit_pad1;
    uint   blit_pad2;
    uint   blit_pad3;
};

struct BlitVSOutput {
    float4 pos : SV_POSITION;
    float2 uv  : TEXCOORD0;
};

BlitVSOutput rgba_blit_vertex(uint vertex_id : SV_VertexID) {
    static const float2 quad[6] = {
        float2(0.0, 0.0),
        float2(1.0, 0.0),
        float2(0.0, 1.0),
        float2(0.0, 1.0),
        float2(1.0, 0.0),
        float2(1.0, 1.0)
    };
    const float2 t = quad[vertex_id];
    BlitVSOutput o;
    o.pos = float4(
        lerp(dst_pos_min.x, dst_pos_max.x, t.x),
        lerp(dst_pos_min.y, dst_pos_max.y, t.y),
        0.0, 1.0);
    o.uv = float2(
        lerp(src_uv_min.x, src_uv_max.x, t.x),
        lerp(src_uv_min.y, src_uv_max.y, t.y));
    return o;
}

Texture2D    blit_src_texture : register(t0);
SamplerState blit_src_sampler : register(s0);

float4 rgba_blit_fragment(BlitVSOutput input) : SV_TARGET {
    return blit_src_texture.Sample(blit_src_sampler, input.uv) * tint_rgba;
}
