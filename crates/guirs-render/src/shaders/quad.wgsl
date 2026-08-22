// Rounded boxes, borders, gradients and shadows.
//
// Every one of those is the same instanced quad. The fragment shader evaluates
// a signed distance field for a rounded rectangle and derives coverage from it,
// which means no tessellation, no CPU path work, and analytic antialiasing that
// stays correct at any scale factor.
//
// All distance math happens in device pixels so that a coverage ramp of one
// unit is exactly one physical pixel wide.

struct Uniforms {
    // Surface size in logical pixels.
    viewport: vec2<f32>,
    // Device pixels per logical pixel.
    scale_factor: f32,
    _pad: f32,
    atlas_sizes: vec4<f32>,
    // Rounded clips, two entries each: bounds, then corner radii. A scissor
    // rectangle cannot describe a corner, so the corners are taken off here.
    clips: array<vec4<f32>, 32>,
    // Transforms, two entries each: the matrix, then the translation and the
    // scale the antialiasing ramp has to be widened by.
    transforms: array<vec4<f32>, 64>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var ramp_texture: texture_2d<f32>;
@group(0) @binding(2) var ramp_sampler: sampler;

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


struct QuadInstance {
    @location(0) bounds: vec4<f32>,
    @location(1) radii: vec4<f32>,
    @location(2) border: vec4<f32>,
    @location(3) background: vec4<f32>,
    @location(4) border_color: vec4<f32>,
    @location(5) shadow: vec4<f32>,
    // kind, fill kind, gradient row, gradient angle
    @location(6) params: vec4<f32>,
    // padding, opacity, rounded clip slot, transform slot
    @location(7) extra: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // Position relative to the shape's center, in device pixels.
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) half_size: vec2<f32>,
    @location(2) @interpolate(flat) radii: vec4<f32>,
    @location(3) @interpolate(flat) border: vec4<f32>,
    @location(4) @interpolate(flat) background: vec4<f32>,
    @location(5) @interpolate(flat) border_color: vec4<f32>,
    @location(6) @interpolate(flat) shadow: vec4<f32>,
    @location(7) @interpolate(flat) params: vec4<f32>,
    @location(8) @interpolate(flat) opacity: f32,
    @location(9) @interpolate(flat) clip_slot: f32,
    // How much the transform magnifies, so coverage can be corrected for it.
    @location(10) @interpolate(flat) aa_scale: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, instance: QuadInstance) -> VertexOutput {
    // Two triangles from six indices, mapped onto the unit square.
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
    let padding = instance.extra.x * scale;

    // Grow the rasterized area so shadows and the antialiasing ramp have room.
    let draw_origin = origin - vec2<f32>(padding, padding);
    let draw_size = size + vec2<f32>(padding * 2.0, padding * 2.0);
    let device_position = draw_origin + corner * draw_size;
    let moved = transform_device(device_position, instance.extra.w);

    let device_viewport = uniforms.viewport * scale;

    var out: VertexOutput;
    out.position = vec4<f32>(
        moved.x / device_viewport.x * 2.0 - 1.0,
        1.0 - moved.y / device_viewport.y * 2.0,
        0.0,
        1.0,
    );
    // Deliberately the untransformed position: the distance field is evaluated
    // in the shape's own space, and the mapping back to it is affine, so
    // interpolating this across the triangle lands exactly.
    out.local = device_position - (origin + size * 0.5);
    out.half_size = size * 0.5;
    out.radii = instance.radii * scale;
    out.border = instance.border * scale;
    out.background = instance.background;
    out.border_color = instance.border_color;
    out.shadow = instance.shadow * scale;
    out.params = instance.params;
    out.opacity = instance.extra.y;
    out.clip_slot = instance.extra.z;
    out.aa_scale = transform_scale(instance.extra.w);
    return out;
}

// Pick the radius for the quadrant a point falls in. Order is top left,
// top right, bottom right, bottom left, with y pointing down.
fn corner_radius(p: vec2<f32>, radii: vec4<f32>) -> f32 {
    let is_left = p.x < 0.0;
    if (p.y < 0.0) {
        return select(radii.y, radii.x, is_left);
    }
    return select(radii.z, radii.w, is_left);
}

// Signed distance to a rounded rectangle centered on the origin. Negative
// inside, positive outside, and in the same units as `p`.
fn sd_rounded_box(p: vec2<f32>, half_size: vec2<f32>, radii: vec4<f32>) -> f32 {
    let r = corner_radius(p, radii);
    let q = abs(p) - half_size + vec2<f32>(r, r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0, 0.0))) - r;
}

// A one pixel wide linear ramp across the edge. Cheaper than fwidth and, at
// device resolution, indistinguishable from it.
fn coverage(distance: f32) -> f32 {
    return clamp(0.5 - distance, 0.0, 1.0);
}

// The same, in a space a transform has magnified. Dividing the distance by how
// much the shape grew keeps the ramp one device pixel wide on screen rather
// than one pre-transform pixel, which under a large scale would go soft.
fn coverage_scaled(distance: f32, magnification: f32) -> f32 {
    return clamp(0.5 - distance / magnification, 0.0, 1.0);
}

// How much of this fragment survives the rounded clip in force, if any.
//
// The straight edges are already gone: the scissor removed them before the
// fragment ever ran. What is left is the corners, and evaluating the same
// signed distance field the shapes use gives them the same antialiasing.
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
    return coverage(sd_rounded_box(p, half_size, radii));
}

// Abramowitz and Stegun 7.1.26, accurate to about 5e-4 across the range that
// matters here.
fn erf_approx(x: f32) -> f32 {
    let s = sign(x);
    let a = abs(x);
    let t = 1.0 + a * (0.278393 + a * (0.230389 + a * (0.000972 + a * 0.078108)));
    let t2 = t * t;
    return s * (1.0 - 1.0 / (t2 * t2));
}

// Coverage of an edge convolved with a Gaussian. This is what makes a shadow
// look like a real blur rather than a feathered outline.
fn blurred_coverage(distance: f32, sigma: f32) -> f32 {
    if (sigma <= 0.0001) {
        return coverage(distance);
    }
    return clamp(0.5 * (1.0 - erf_approx(distance / (sigma * 1.4142135))), 0.0, 1.0);
}

fn adjust_radii(radii: vec4<f32>, delta: f32) -> vec4<f32> {
    return max(radii + vec4<f32>(delta, delta, delta, delta), vec4<f32>(0.0, 0.0, 0.0, 0.0));
}

fn gradient_position(p: vec2<f32>, half_size: vec2<f32>, fill_kind: f32, angle: f32) -> f32 {
    if (fill_kind > 1.5) {
        // Radial, reaching the farthest corner at 1.0.
        return clamp(length(p) / max(length(half_size), 0.0001), 0.0, 1.0);
    }
    // Zero degrees points up and the angle increases clockwise, matching CSS.
    let direction = vec2<f32>(sin(angle), -cos(angle));
    let line_length =
        abs(2.0 * half_size.x * direction.x) + abs(2.0 * half_size.y * direction.y);
    return clamp(dot(p, direction) / max(line_length, 0.0001) + 0.5, 0.0, 1.0);
}

fn sample_fill(in: VertexOutput) -> vec4<f32> {
    let fill_kind = in.params.y;
    if (fill_kind < 0.5) {
        return in.background;
    }
    let t = gradient_position(in.local, in.half_size, fill_kind, in.params.w);
    let rows = f32(textureDimensions(ramp_texture).y);
    let v = (in.params.z + 0.5) / max(rows, 1.0);
    return textureSampleLevel(ramp_texture, ramp_sampler, vec2<f32>(t, v), 0.0);
}

// Straight alpha in, premultiplied out.
fn premultiply(color: vec4<f32>, alpha: f32) -> vec4<f32> {
    let a = color.a * alpha;
    return vec4<f32>(color.rgb * a, a);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let kind = in.params.x;
    let sigma = in.shadow.z * 0.5;
    let offset = in.shadow.xy;
    let spread = in.shadow.w;

    let clipped = clip_coverage(in.position.xy, in.clip_slot);
    if (clipped <= 0.0) {
        discard;
    }

    if (kind > 1.5) {
        // Inset shadow: the blurred shape is the hole, so coverage inverts,
        // and the whole thing is clipped to the box.
        let inner_half = max(in.half_size - vec2<f32>(spread, spread), vec2<f32>(0.0, 0.0));
        let d = sd_rounded_box(in.local - offset, inner_half, adjust_radii(in.radii, -spread));
        let shadow_alpha = 1.0 - blurred_coverage(d, sigma);
        let inside = coverage(sd_rounded_box(in.local, in.half_size, in.radii));
        return premultiply(in.background, shadow_alpha * inside * in.opacity * clipped);
    }

    if (kind > 0.5) {
        // Drop shadow.
        let outer_half = in.half_size + vec2<f32>(spread, spread);
        let d = sd_rounded_box(in.local - offset, outer_half, adjust_radii(in.radii, spread));
        return premultiply(in.background, blurred_coverage(d, sigma) * in.opacity * clipped);
    }

    // A filled, bordered box.
    let outer_distance = sd_rounded_box(in.local, in.half_size, in.radii);
    let outer = coverage_scaled(outer_distance, in.aa_scale);
    if (outer <= 0.0) {
        discard;
    }

    var color = premultiply(sample_fill(in), outer);

    let border = in.border;
    let has_border = border.x + border.y + border.z + border.w > 0.0;
    if (has_border && in.border_color.a > 0.0) {
        // The content box sits inside the border, and is not centered when the
        // four widths differ.
        let inner_half = max(
            in.half_size - vec2<f32>((border.y + border.w) * 0.5, (border.x + border.z) * 0.5),
            vec2<f32>(0.0, 0.0),
        );
        let inner_center = vec2<f32>((border.w - border.y) * 0.5, (border.x - border.z) * 0.5);
        let thickest = max(max(border.x, border.y), max(border.z, border.w));
        let inner_distance = sd_rounded_box(
            in.local - inner_center,
            inner_half,
            adjust_radii(in.radii, -thickest),
        );
        let ring = clamp(outer - coverage_scaled(inner_distance, in.aa_scale), 0.0, 1.0);
        let stroke = premultiply(in.border_color, ring);
        color = stroke + color * (1.0 - stroke.a);
    }

    return color * in.opacity * clipped;
}
