cbuffer Uniforms : register(b0) {
    float viewport_width;
    float viewport_height;
    float cell_width;
    float cell_height;
    float atlas_width;
    float atlas_height;
    float row_origin;
    float effects_mode;
};

struct VSInput {
    float2 pos : POSITION;
    float2 uv : TEXCOORD0;
    float4 fg : COLOR0;
    float4 bg : COLOR1;
    uint flags : BLENDINDICES0;
    uint vertex_id : SV_VertexID;
};

struct VSOutput {
    float4 pos : SV_POSITION;
    float2 uv : TEXCOORD0;
    float2 screen_uv : TEXCOORD1;
    float4 fg : COLOR0;
    float4 bg : COLOR1;
};

VSOutput glyph_vertex(VSInput input) {
    static const float2 quad[6] = {
        float2(0.0, 0.0),
        float2(1.0, 0.0),
        float2(0.0, 1.0),
        float2(0.0, 1.0),
        float2(1.0, 0.0),
        float2(1.0, 1.0)
    };

    const float2 corner = quad[input.vertex_id];
    const float2 pixel = float2(
        input.pos.x * cell_width,
        (input.pos.y - row_origin) * cell_height
    ) + corner * float2(cell_width, cell_height);
    const float2 ndc = float2(
        (pixel.x / viewport_width) * 2.0 - 1.0,
        1.0 - (pixel.y / viewport_height) * 2.0
    );

    VSOutput output;
    output.pos = float4(ndc, 0.0, 1.0);
    output.uv = (input.uv + corner * float2(cell_width, cell_height)) / float2(atlas_width, atlas_height);
    output.screen_uv = float2(
        pixel.x / max(viewport_width, 1.0),
        pixel.y / max(viewport_height, 1.0)
    );
    output.fg = input.fg;
    output.bg = input.bg;
    return output;
}

Texture2D atlas_texture : register(t0);
SamplerState atlas_sampler : register(s0);

float3 apply_crt_effect(float3 colour, float2 screen_uv, float mode) {
    const float2 centered = screen_uv * 2.0 - 1.0;
    const float vignette = saturate(1.08 - dot(centered * float2(0.92, 1.15), centered) * 0.55);
    const float scanline = 0.90 + 0.10 * sin(screen_uv.y * viewport_height * 3.14159265);
    return saturate(colour * vignette * scanline);
}

float4 glyph_fragment(VSOutput input) : SV_TARGET {
    const float alpha = atlas_texture.Sample(atlas_sampler, input.uv).a;
    float4 colour = lerp(input.bg, input.fg, alpha);
    if (effects_mode > 0.5) {
        colour.rgb = apply_crt_effect(colour.rgb, input.screen_uv, effects_mode);
    }
    return colour;
}