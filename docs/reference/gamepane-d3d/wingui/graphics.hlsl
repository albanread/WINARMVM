cbuffer GraphicsDisplay : register(b0) {
    float uv_scale_x;
    float uv_scale_y;
    float uv_offset_x;
    float uv_offset_y;
    uint screen_width;
    uint screen_height;
    uint buffer_width;
    uint buffer_height;
    int scroll_x;
    int scroll_y;
    uint has_texture;
    uint graphics_pad0;
};

struct GraphicsVSOutput {
    float4 pos : SV_POSITION;
    float2 uv : TEXCOORD0;
};

GraphicsVSOutput graphics_vertex(uint vertex_id : SV_VertexID) {
    static const float2 quad[6] = {
        float2(0.0, 0.0),
        float2(1.0, 0.0),
        float2(0.0, 1.0),
        float2(0.0, 1.0),
        float2(1.0, 0.0),
        float2(1.0, 1.0)
    };

    const float2 local = quad[vertex_id];
    GraphicsVSOutput output;
    output.pos = float4(local.x * 2.0 - 1.0, 1.0 - local.y * 2.0, 0.0, 1.0);
    output.uv = float2(
        (local.x - uv_offset_x) / max(uv_scale_x, 0.0001),
        (local.y - uv_offset_y) / max(uv_scale_y, 0.0001));
    return output;
}

Texture2D rgba_texture : register(t0);
SamplerState rgba_sampler : register(s0);

Texture2D<uint4> graphics_indexed_texture : register(t0);
Texture2D<uint> graphics_line_palette : register(t1);
Texture2D<uint> graphics_global_palette : register(t2);

float4 unpack_packed_rgba(uint packed) {
    const float r = (float)(packed & 0xFFu);
    const float g = (float)((packed >> 8) & 0xFFu);
    const float b = (float)((packed >> 16) & 0xFFu);
    const float a = (float)((packed >> 24) & 0xFFu);
    return float4(r, g, b, a) / 255.0;
}

int wrap_graphics_coord(int value, uint size) {
    const int span = max((int)size, 1);
    int wrapped = value % span;
    if (wrapped < 0) wrapped += span;
    return wrapped;
}

float4 graphics_fragment(GraphicsVSOutput input) : SV_TARGET {
    if (has_texture == 0) return float4(0.0, 0.0, 0.0, 1.0);
    if (input.uv.x < 0.0 || input.uv.x > 1.0 || input.uv.y < 0.0 || input.uv.y > 1.0) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }

    const uint2 display_pos = min(
        uint2(input.uv * float2((float)screen_width, (float)screen_height)),
        uint2(max((int)screen_width - 1, 0), max((int)screen_height - 1, 0)));

    const int src_x = wrap_graphics_coord((int)display_pos.x + scroll_x, buffer_width);
    const int src_y = wrap_graphics_coord((int)display_pos.y + scroll_y, buffer_height);
    const uint index = graphics_indexed_texture.Load(int3(src_x, src_y, 0)).r;

    if (index == 0) {
        return float4(0.0, 0.0, 0.0, 0.0);
    }
    if (index < 16) {
        const int palette_y = min((int)display_pos.y, max((int)buffer_height - 1, 0));
        return unpack_packed_rgba(graphics_line_palette.Load(int3((int)index, palette_y, 0)));
    }
    return unpack_packed_rgba(graphics_global_palette.Load(int3((int)index - 16, 0, 0)));
}

float4 rgba_fragment(GraphicsVSOutput input) : SV_TARGET {
    if (has_texture == 0) return float4(0.0, 0.0, 0.0, 1.0);
    if (input.uv.x < 0.0 || input.uv.x > 1.0 || input.uv.y < 0.0 || input.uv.y > 1.0) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    return rgba_texture.Sample(rgba_sampler, input.uv);
}