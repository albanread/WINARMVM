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

struct SpriteVSInput {
    float2 pos : POSITION;
    float2 atlas_px : TEXCOORD0;
    float palette_slot : TEXCOORD1;
    float alpha : TEXCOORD2;
    float effect_type : TEXCOORD3;
    float effect_param1 : TEXCOORD4;
    float effect_param2 : TEXCOORD5;
    float4 effect_colour : COLOR0;
};

struct SpritePSInput {
    float4 pos : SV_POSITION;
    float2 atlas_px : TEXCOORD0;
    float palette_slot : TEXCOORD1;
    float alpha : TEXCOORD2;
    float effect_type : TEXCOORD3;
    float effect_param1 : TEXCOORD4;
    float effect_param2 : TEXCOORD5;
    float4 effect_colour : COLOR0;
};

SpritePSInput sprite_vertex(SpriteVSInput input) {
    SpritePSInput output;
    output.pos = float4(
        (input.pos.x / max(viewport_width, 1.0)) * 2.0 - 1.0,
        1.0 - (input.pos.y / max(viewport_height, 1.0)) * 2.0,
        0.0,
        1.0);
    output.atlas_px = input.atlas_px;
    output.palette_slot = input.palette_slot;
    output.alpha = input.alpha;
    output.effect_type = input.effect_type;
    output.effect_param1 = input.effect_param1;
    output.effect_param2 = input.effect_param2;
    output.effect_colour = input.effect_colour;
    return output;
}

Texture2D<uint4> sprite_atlas : register(t0);
Texture2D<uint> sprite_palette : register(t1);

float4 unpack_packed_rgba(uint packed) {
    const float r = (float)(packed & 0xFFu);
    const float g = (float)((packed >> 8) & 0xFFu);
    const float b = (float)((packed >> 16) & 0xFFu);
    const float a = (float)((packed >> 24) & 0xFFu);
    return float4(r, g, b, a) / 255.0;
}

float4 sprite_fragment(SpritePSInput input) : SV_TARGET {
    const uint index = sprite_atlas.Load(int3(int(round(input.atlas_px.x)), int(round(input.atlas_px.y)), 0)).r;
    if (index == 0) discard;

    float4 colour = unpack_packed_rgba(sprite_palette.Load(int3(int(index), int(input.palette_slot), 0)));
    colour.a *= input.alpha;

    if (int(input.effect_type) == 1) {
        colour.rgb = saturate(colour.rgb + input.effect_colour.rgb * (input.effect_param2 * 0.3));
    } else if (int(input.effect_type) == 4) {
        colour.rgb = lerp(colour.rgb, colour.rgb * input.effect_colour.rgb, saturate(input.effect_param1));
    } else if (int(input.effect_type) == 5) {
        const float wave = frac(input.effect_param1 * 0.1);
        colour.rgb = lerp(colour.rgb, input.effect_colour.rgb, step(0.5, wave));
    }
    return colour;
}