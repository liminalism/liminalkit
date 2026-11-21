// liminalkit-game: one layer drawn through its warp mesh, with its effects.
//
// The `Layer` uniform struct is prepended by `liminalkit_game::gpu::layer_wgsl`
// from the same table the Rust side packs its bytes from.
//
// Texture: the layer's buffer exactly as painted — premultiplied alpha, colour
// in encoded sRGB — in a Bgra8Unorm texture, sampled linear, clamp to edge.
// Blend: premultiplied (ONE, ONE_MINUS_SRC_ALPHA). No depth, no culling.
//
// `lk_shade` is `liminalkit_game::effect::shade`, line for line; `lk_premul`
// is `liminalkit_raster`'s textured-mesh pixel. Keep them so.

@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var layer_tex: texture_2d<f32>;
@group(0) @binding(2) var layer_samp: sampler;

struct VsIn {
    @location(0) pos: vec2f,    // device pixels, top-left origin
    @location(1) uv: vec2f,     // texels of the texture's largest level
    @location(2) attrs: vec4f,  // alpha, px_per_texel, lod, light
};

struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) @interpolate(linear) uv: vec2f,
    @location(1) @interpolate(linear) attrs: vec4f,
};

@vertex
fn vs_layer(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = vec4f(
        v.pos.x / layer.size.x * 2.0 - 1.0,
        1.0 - v.pos.y / layer.size.y * 2.0,
        0.0,
        1.0,
    );
    o.uv = v.uv;
    o.attrs = v.attrs;
    return o;
}

// Signed distance to a rounded rectangle (left, top, right, bottom).
fn lk_rounded(p: vec2f, rect: vec4f, corner_in: f32) -> f32 {
    let half = (rect.zw - rect.xy) * 0.5;
    let centre = (rect.xy + rect.zw) * 0.5;
    let corner = max(min(corner_in, min(half.x, half.y)), 0.0);
    let q = abs(p - centre) - (half - vec2f(corner));
    return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - corner;
}

// Move a premultiplied pixel toward `color` by `amount` of its own alpha.
fn lk_brighten(p: vec4f, color: vec3f, amount: f32) -> vec4f {
    return vec4f(min(p.rgb + color * amount * p.a, vec3f(p.a)), p.a);
}

fn lk_shade(src: vec4f, texel: vec2f) -> vec4f {
    var p = src;

    if (layer.glow_color.a > 0.0) {
        let distance = lk_rounded(texel, layer.glow_rect, layer.glow_shape.x);
        let radius = layer.glow_shape.y;
        let outline = layer.glow_shape.z;
        var level = 0.0;
        if (radius > 0.0 && distance > 0.0) {
            let fall = clamp(1.0 - distance / radius, 0.0, 1.0);
            level = fall * fall;
        }
        if (outline > 0.0) {
            level = max(level, clamp(1.0 - abs(distance) / (outline * 0.5), 0.0, 1.0));
        }
        let alpha = clamp(level * layer.glow_color.a, 0.0, 1.0);
        let under = 1.0 - p.a;
        p = vec4f(p.rgb + under * layer.glow_color.rgb * alpha, p.a + under * alpha);
    }

    if (layer.shimmer_color.a > 0.0) {
        let along = dot(layer.shimmer_line.xy, texel) - layer.shimmer_line.z;
        let across = along / layer.shimmer_line.w;
        let bell = max(1.0 - across * across, 0.0);
        p = lk_brighten(p, layer.shimmer_color.rgb, layer.shimmer_color.a * bell * bell);
    }

    if (layer.flash.a > 0.0) {
        p = lk_brighten(p, layer.flash.rgb, layer.flash.a);
    }

    if (layer.scan.y > 0.0 && layer.scan.x > 0.0) {
        let turn = 6.283185307179586 * texel.y / layer.scan.x + layer.scan.z;
        p = p * (1.0 - layer.scan.y * 0.5 * (1.0 + cos(turn)));
    }

    return p;
}

fn lk_premul(i: VsOut) -> vec4f {
    let size = layer.size.zw;
    // Signed distance, in texels, inside the texture's edge; one pixel of
    // ramp across it is the layer's anti-aliasing.
    let inset = min(min(i.uv.x, size.x - i.uv.x), min(i.uv.y, size.y - i.uv.y));
    let coverage = clamp(0.5 + inset * i.attrs.y, 0.0, 1.0);
    let weight = clamp(i.attrs.x, 0.0, 1.0) * coverage * layer.params.x;
    let lod = clamp(i.attrs.z, 0.0, layer.params.z);
    let sampled = textureSampleLevel(layer_tex, layer_samp, i.uv / size, lod);
    var p = lk_shade(sampled, i.uv) * weight;
    let light = i.attrs.w;
    if (light < 0.0) {
        p = vec4f(p.rgb * (1.0 + max(light, -1.0)), p.a);
    } else if (light > 0.0 && layer.params.y > 0.0) {
        let rim = clamp(1.0 - inset * i.attrs.y / layer.params.y, 0.0, 1.0);
        p = vec4f(min(p.rgb + vec3f(min(light, 1.0) * rim * p.a), vec3f(p.a)), p.a);
    }
    return p;
}

// For a UNORM target: blend the encoded values, as liminalkit itself does.
@fragment
fn fs_layer_encoded(i: VsOut) -> @location(0) vec4f {
    return lk_premul(i);
}

fn lk_decode(c: vec3f) -> vec3f {
    let low = c / 12.92;
    let high = pow((c + vec3f(0.055)) / 1.055, vec3f(2.4));
    return select(high, low, c <= vec3f(0.04045));
}

// For an sRGB target, which blends in linear light: decode the colour and
// correct the coverage (a' = 1 - (1 - a)^2.2) so that dark glass over a
// bright scene lets through what an encoded blend would have.
@fragment
fn fs_layer_linear(i: VsOut) -> @location(0) vec4f {
    let p = lk_premul(i);
    if (p.a <= 0.0) {
        return vec4f(0.0);
    }
    let colour = clamp(p.rgb / p.a, vec3f(0.0), vec3f(1.0));
    let alpha = 1.0 - pow(1.0 - min(p.a, 1.0), 2.2);
    return vec4f(lk_decode(colour) * alpha, alpha);
}
