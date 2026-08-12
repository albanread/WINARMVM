// Compute shader for writing indexed-colour rectangles directly into a
// WinguiIndexedSurface pixel texture (DXGI_FORMAT_R8_UINT UAV).
//
// Dispatch threadgroups of (8,8,1) covering the destination rect.
// Threads outside the rect do nothing.

cbuffer IndexedFillParams : register(b0) {
    uint dst_x;          // destination rect left (pixels)
    uint dst_y;          // destination rect top  (pixels)
    uint dst_w;          // destination rect width
    uint dst_h;          // destination rect height
    uint palette_index;  // 0-255; 0 = transparent in the renderer
    uint pad0;
    uint pad1;
    uint pad2;
};

RWTexture2D<uint> dst_pixels : register(u0);

[numthreads(8, 8, 1)]
void indexed_fill_cs(uint3 tid : SV_DispatchThreadID) {
    if (tid.x >= dst_w || tid.y >= dst_h) return;
    dst_pixels[uint2(dst_x + tid.x, dst_y + tid.y)] = palette_index;
}

// ---------------------------------------------------------------------------
// Line drawing — one thread per step along the major axis.
// Dispatch (ceil(steps+1, 64), 1, 1) threadgroups.
// Uses lerp+round for integer-exact DDA; produces a 1px-wide line.
// ---------------------------------------------------------------------------

cbuffer IndexedLineParams : register(b1) {
    int  line_x0;        // start x
    int  line_y0;        // start y
    int  line_x1;        // end x
    int  line_y1;        // end y
    uint line_palette;   // palette index to write
    uint line_steps;     // max(|dx|,|dy|) — precomputed on CPU
    uint line_pad0;
    uint line_pad1;
};

[numthreads(64, 1, 1)]
void indexed_line_cs(uint3 tid : SV_DispatchThreadID) {
    if (tid.x > line_steps) return;  // strict >, so thread 0 draws start pixel, thread steps draws end pixel
    float t  = (line_steps > 0) ? (float)tid.x / (float)line_steps : 0.0f;
    int   px = (int)round(lerp((float)line_x0, (float)line_x1, t));
    int   py = (int)round(lerp((float)line_y0, (float)line_y1, t));
    dst_pixels[uint2((uint)px, (uint)py)] = line_palette;
}
