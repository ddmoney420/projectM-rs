// Effect übershader: one pipeline, effect selected by `u.mode`. ALL sampling
// uses textureSampleLevel (LOD 0, no implicit derivatives) so multi-tap loops
// and mode branches are always valid regardless of control-flow uniformity.

struct U {
    resolution: vec2<f32>,
    mode: f32,
    time: f32,
    p: vec4<f32>,   // p0..p3
    p2: vec2<f32>,  // p4, p5
    pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var inp: texture_2d<f32>;
@group(0) @binding(2) var aux: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;

@vertex
fn vs(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
    var pp = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(pp[vid], 0.0, 1.0);
}

fn samp(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(inp, smp, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
}
fn sampa(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(aux, smp, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
}
fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}
fn hue_rotate(c: vec3<f32>, h: f32) -> vec3<f32> {
    let a = h * 6.2831853;
    let ca = cos(a);
    let sa = sin(a);
    let m = mat3x3<f32>(
        vec3<f32>(0.299 + 0.701 * ca + 0.168 * sa, 0.587 - 0.587 * ca + 0.330 * sa, 0.114 - 0.114 * ca - 0.497 * sa),
        vec3<f32>(0.299 - 0.299 * ca - 0.328 * sa, 0.587 + 0.413 * ca + 0.035 * sa, 0.114 - 0.114 * ca + 0.292 * sa),
        vec3<f32>(0.299 - 0.300 * ca + 1.250 * sa, 0.587 - 0.588 * ca - 1.050 * sa, 0.114 + 0.886 * ca - 0.203 * sa),
    );
    return clamp(m * c, vec3<f32>(0.0), vec3<f32>(1.0));
}
fn kaleido(uv: vec2<f32>, seg: f32, rot: f32) -> vec2<f32> {
    let n = max(seg, 1.0);
    let p = uv - vec2<f32>(0.5);
    var a = atan2(p.y, p.x) + rot;
    let r = length(p);
    let seg_ang = 6.2831853 / n;
    a = abs((a % seg_ang) - seg_ang * 0.5);
    return vec2<f32>(cos(a), sin(a)) * r + vec2<f32>(0.5);
}

@fragment
fn fs(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let res = u.resolution;
    let uv = frag.xy / res;
    let m = i32(u.mode + 0.5);
    let texel = vec2<f32>(1.0) / res;
    var col = vec3<f32>(0.0);

    if (m == 0) { col = clamp(samp(uv).rgb + vec3<f32>(u.p.x), vec3<f32>(0.0), vec3<f32>(1.0)); }
    else if (m == 1) { col = clamp((samp(uv).rgb - vec3<f32>(0.5)) * u.p.x + vec3<f32>(0.5), vec3<f32>(0.0), vec3<f32>(1.0)); }
    else if (m == 2) { let c = samp(uv).rgb; let g = dot(c, vec3<f32>(0.299, 0.587, 0.114)); col = clamp(mix(vec3<f32>(g), c, u.p.x), vec3<f32>(0.0), vec3<f32>(1.0)); }
    else if (m == 3) { col = hue_rotate(samp(uv).rgb, u.p.x); }
    else if (m == 4) { let c = samp(uv).rgb; col = mix(c, vec3<f32>(1.0) - c, u.p.x); }
    else if (m == 5) { let lv = max(u.p.x, 2.0); col = floor(samp(uv).rgb * lv) / (lv - 1.0); }
    else if (m == 6) { col = samp(vec2<f32>(0.5 - abs(uv.x - 0.5), uv.y)).rgb; }
    else if (m == 7) { col = samp(vec2<f32>(uv.x, 0.5 - abs(uv.y - 0.5))).rgb; }
    else if (m == 8) { col = samp(kaleido(uv, u.p.x, u.p.y)).rgb; }
    else if (m == 9) {
        let n = max(u.p.x, 1.0);
        let p = uv - vec2<f32>(0.5);
        var a = atan2(p.y, p.x) + u.p.y;
        let r = length(p);
        let sa = 6.2831853 / n;
        a = a % sa;
        col = samp(vec2<f32>(cos(a), sin(a)) * r + vec2<f32>(0.5)).rgb;
    }
    else if (m == 10) { let s = max(u.p.x, 2.0); col = samp((floor(uv * s) + vec2<f32>(0.5)) / s).rgb; }
    else if (m == 11) {
        let r = clamp(u.p.x, 0.0, 16.0);
        let dir = select(vec2<f32>(texel.x, 0.0), vec2<f32>(0.0, texel.y), u.p2.y > 0.5);
        var sum = vec3<f32>(0.0);
        var wsum = 0.0;
        for (var i = -16; i <= 16; i = i + 1) {
            let fi = f32(i);
            if (abs(fi) <= r) {
                let w = exp(-(fi * fi) / (max(r * r, 1.0) * 0.5 + 0.001));
                sum = sum + samp(uv + dir * fi).rgb * w;
                wsum = wsum + w;
            }
        }
        col = sum / max(wsum, 0.001);
    }
    else if (m == 12) {
        let c = samp(uv).rgb;
        let blur = (samp(uv + vec2<f32>(texel.x, 0.0)).rgb + samp(uv - vec2<f32>(texel.x, 0.0)).rgb
                  + samp(uv + vec2<f32>(0.0, texel.y)).rgb + samp(uv - vec2<f32>(0.0, texel.y)).rgb) * 0.25;
        col = clamp(c + (c - blur) * u.p.x, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    else if (m == 13) {
        let gx = samp(uv + vec2<f32>(texel.x, 0.0)).rgb - samp(uv - vec2<f32>(texel.x, 0.0)).rgb;
        let gy = samp(uv + vec2<f32>(0.0, texel.y)).rgb - samp(uv - vec2<f32>(0.0, texel.y)).rgb;
        col = clamp(sqrt(gx * gx + gy * gy) * u.p.x, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    else if (m == 14) {
        let d = distance(uv, vec2<f32>(0.5)) * 1.41421;
        let v = smoothstep(1.0, 1.0 - max(u.p.y, 0.01), d * u.p.x);
        col = samp(uv).rgb * v;
    }
    else if (m == 15) {
        let n = hash(uv * res + vec2<f32>(u.time)) - 0.5;
        col = clamp(samp(uv).rgb + vec3<f32>(n * u.p.x), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    else if (m == 16) {
        let s = 0.5 + 0.5 * sin(uv.y * u.p.y);
        col = samp(uv).rgb * (1.0 - u.p.x * (1.0 - s));
    }
    else if (m == 17) {
        let o = (uv - vec2<f32>(0.5)) * u.p.x;
        col = vec3<f32>(samp(uv + o).r, samp(uv).g, samp(uv - o).b);
    }
    else if (m == 18) {
        let dir = vec2<f32>(cos(u.p.y), sin(u.p.y)) * u.p.x;
        col = vec3<f32>(samp(uv + dir).r, samp(uv).g, samp(uv - dir).b);
    }
    else if (m == 19) {
        let row = floor(uv.y * 24.0);
        let jitter = (hash(vec2<f32>(row, floor(u.time * 12.0))) - 0.5) * u.p.x * 0.2;
        let on = step(0.6, hash(vec2<f32>(row, floor(u.time * 8.0) + 3.0)));
        col = samp(uv + vec2<f32>(jitter * on, 0.0)).rgb;
    }
    else if (m == 20) {
        let amount = u.p.x;
        let zoom = max(u.p.y, 0.01);
        let rot = u.p.z;
        let off = vec2<f32>(u.p.w, u.p2.x);
        var q = (uv - vec2<f32>(0.5)) / zoom;
        let ca = cos(rot);
        let sa = sin(rot);
        q = vec2<f32>(ca * q.x - sa * q.y, sa * q.x + ca * q.y) + vec2<f32>(0.5) + off;
        col = max(samp(uv).rgb, sampa(q).rgb * amount);
    }
    else if (m == 21) {
        col = max(samp(uv).rgb - vec3<f32>(u.p.x), vec3<f32>(0.0));
    }
    else if (m == 22) {
        col = clamp(samp(uv).rgb + sampa(uv).rgb * u.p.x, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    else {
        col = samp(uv).rgb;
    }

    return vec4<f32>(col, 1.0);
}
