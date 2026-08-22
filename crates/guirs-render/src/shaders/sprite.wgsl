// Textured rectangles: glyphs and images.
//
// Three sources share one pipeline. Alpha glyphs sample a single channel
// coverage mask and take their color from the instance; color glyphs and images
// sample full RGBA. Keeping them together means a line of text with an inline
// emoji is still a single draw call.

struct Uniforms {
    viewport: vec2<f32>,
    scale_factor: f32,
    _pad: f32,
    atlas_sizes: vec4<f32>,
    // Rounded clips, two entries each: bounds, then corner radii.
    clips: array<vec4<f32>, 32>,
    // Transforms, two entries each: the matrix, then the translation and the
    // magnification.
    transforms: array<vec4<f32>, 64>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var mono_atlas: texture_2d_array<f32>;
@group(1) @binding(1) var color_atlas: texture_2d_array<f32>;
// Pictures get their own atlas rather than sharing the one color glyphs use.
// Emoji are small and images are not, so one page size cannot suit both: a
// page large enough for a photograph wastes most of itself on emoji, and a
// page sized for emoji cannot hold a photograph at all.
@group(1) @binding(3) var image_atlas: texture_2d_array<f32>;
@group(1) @binding(2) var atlas_sampler: sampler;

// A transform, if this instance carries one. The matrix works in device pixels
// and is applied to the vertex position only: the fragment keeps its own local
// coordinate, so the distance field is evaluated in the shape's own space and
// stays exact however the shape has been moved.
fn transform_device(position: vec2<f32>, slot: f32) -> vec2<f32> {
    if (slot < 0.0) {
        return position;
    }
    let index = u32(slot) * 2u;
    let matrix = uniforms.transforms[index];
    let offset = uniforms.transforms[index + 1u];
    let scale = uniforms.scale_factor;
    // The translation is authored in logical pixels; the rest is scale free.
    return vec2<f32>(
        matrix.x * position.x + matrix.z * position.y + offset.x * scale,
        matrix.y * position.x + matrix.w * position.y + offset.y * scale,
    );
}

// How much a transform magnifies lengths, so the coverage ramp can be widened
// to match and a scaled edge stays exactly one device pixel soft.
fn transform_scale(slot: f32) -> f32 {
    if (slot < 0.0) {
        return 1.0;
    }
    return max(uniforms.transforms[u32(slot) * 2u + 1u].z, 0.0001);
}


struct SpriteInstance {
    @location(0) bounds: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    // kind, corner radius, opacity, atlas layer
    @location(3) params: vec4<f32>,
    // rounded clip slot, transform slot, reserved, reserved
    @location(4) extra: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) local: vec2<f32>,
    @location(2) @interpolate(flat) half_size: vec2<f32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) params: vec4<f32>,
    @location(5) @interpolate(flat) clip_slot: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, instance: SpriteInstance) -> VertexOutput {
    var corner_index = vertex_index;
    if (corner_index == 3u) {
        corner_index = 2u;
    } else if (corner_index == 4u) {
        corner_index = 1u;
    } else if (corner_index == 5u) {
        corner_index = 3u;
    }
    let corner = vec2<f32>(
        f32(corner_index & 1u),
        f32((corner_index >> 1u) & 1u),
    );

    let scale = uniforms.scale_factor;
    let origin = instance.bounds.xy * scale;
    let size = instance.bounds.zw * scale;
    let device_position = origin + corner * size;
    let moved = transform_device(device_position, instance.extra.y);
    let device_viewport = uniforms.viewport * scale;

    var out: VertexOutput;
    out.position = vec4<f32>(
        moved.x / device_viewport.x * 2.0 - 1.0,
        1.0 - moved.y / device_viewport.y * 2.0,
        0.0,
        1.0,
    );
    out.uv = mix(instance.uv.xy, instance.uv.zw, corner);
    out.local = device_position - (origin + size * 0.5);
    out.half_size = size * 0.5;
    out.color = instance.color;
    out.params = instance.params;
    out.clip_slot = instance.extra.x;
    return out;
}

fn sd_rounded_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let r = min(radius, min(half_size.x, half_size.y));
    let q = abs(p) - half_size + vec2<f32>(r, r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0, 0.0))) - r;
}

// Per corner, for the clip, which can have four different radii.
fn sd_rounded_box_4(p: vec2<f32>, half_size: vec2<f32>, radii: vec4<f32>) -> f32 {
    let is_left = p.x < 0.0;
    var r: f32;
    if (p.y < 0.0) {
        r = select(radii.y, radii.x, is_left);
    } else {
        r = select(radii.z, radii.w, is_left);
    }
    let q = abs(p) - half_size + vec2<f32>(r, r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0, 0.0))) - r;
}

// How much of this fragment survives the rounded clip in force, if any. The
// scissor has already taken the straight edges; this is the corners.
fn clip_coverage(device_position: vec2<f32>, slot: f32) -> f32 {
    if (slot < 0.0) {
        return 1.0;
    }
    let index = u32(slot) * 2u;
    let scale = uniforms.scale_factor;
    let bounds = uniforms.clips[index];
    let radii = uniforms.clips[index + 1u] * scale;

    let origin = bounds.xy * scale;
    let half_size = bounds.zw * scale * 0.5;
    let p = device_position - (origin + half_size);
    return clamp(0.5 - sd_rounded_box_4(p, half_size, radii), 0.0, 1.0);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let kind = in.params.x;
    let radius = in.params.y * uniforms.scale_factor;
    let opacity = in.params.z;
    let layer = i32(in.params.w);

    var color: vec4<f32>;
    if (kind < 0.5) {
        // Alpha glyph: the atlas stores coverage, the instance stores the ink.
        let mask = textureSampleLevel(mono_atlas, atlas_sampler, in.uv, layer, 0.0).r;
        let a = in.color.a * mask * opacity;
        color = vec4<f32>(in.color.rgb * a, a);
    } else if (kind < 1.5) {
        // Color glyph. Atlas contents are straight alpha.
        let texel = textureSampleLevel(color_atlas, atlas_sampler, in.uv, layer, 0.0);
        let tinted = texel * in.color;
        let a = tinted.a * opacity;
        color = vec4<f32>(tinted.rgb * a, a);
    } else {
        // A picture, from its own atlas.
        let texel = textureSampleLevel(image_atlas, atlas_sampler, in.uv, layer, 0.0);
        let tinted = texel * in.color;
        let a = tinted.a * opacity;
        color = vec4<f32>(tinted.rgb * a, a);
    }

    // Rounding applies to images, where an avatar or thumbnail wants the same
    // corner treatment as the box behind it. Glyphs never ask for it.
    if (radius > 0.0) {
        let d = sd_rounded_box(in.local, in.half_size, radius);
        color = color * clamp(0.5 - d, 0.0, 1.0);
    }

    return color * clip_coverage(in.position.xy, in.clip_slot);
}
