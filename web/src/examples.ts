// Original example shaders for the live console. These are authored for this
// project (licensed under the repository's LGPL-2.1) — NOT copied from Shadertoy
// or any third party. They demonstrate the Shadertoy-style contract and the
// projectM-derived audio inputs (see the prelude docs in crates/pm-glsl).

export interface Example {
  name: string;
  mode: 'shadertoy' | 'raw';
  source: string;
}

export const EXAMPLES: Example[] = [
  {
    name: 'Animated plasma',
    mode: 'shadertoy',
    source: `// Original example (LGPL-2.1). Time-driven plasma — no audio.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 p = (fragCoord / iResolution.xy) * 8.0;
    float v = sin(p.x + iTime)
            + sin(p.y + iTime * 0.7)
            + sin((p.x + p.y) * 0.5 + iTime)
            + sin(length(p - 4.0) - iTime);
    vec3 col = 0.5 + 0.5 * cos(vec3(0.0, 2.0, 4.0) + v);
    fragColor = vec4(col, 1.0);
}`,
  },
  {
    name: 'Radial spectrum',
    mode: 'shadertoy',
    source: `// Original example (LGPL-2.1). FFT ring from iChannel0 row y=0.25.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;
    float r = length(uv);
    float a = atan(uv.y, uv.x) / 6.2831853 + 0.5;
    float fft = texture(iChannel0, vec2(a, 0.25)).x;
    float ring = smoothstep(0.03, 0.0, abs(r - 0.25 - fft * 0.35));
    vec3 col = ring * (0.5 + 0.5 * cos(vec3(0.0, 2.0, 4.0) + a * 6.2831853));
    fragColor = vec4(col, 1.0);
}`,
  },
  {
    name: 'Bass vortex',
    mode: 'shadertoy',
    source: `// Original example (LGPL-2.1). Tunnel depth driven by iBass.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;
    float r = length(uv);
    float a = atan(uv.y, uv.x);
    float depth = 0.3 / r + iTime * 0.5 + iBass * 2.0;
    float rings = 0.5 + 0.5 * sin(depth * 6.2831853);
    float spokes = 0.5 + 0.5 * sin(a * 8.0 + iTime);
    vec3 col = mix(vec3(0.1, 0.0, 0.2), vec3(0.9, 0.4, 1.0), rings * spokes);
    fragColor = vec4(col * clamp(r * 2.0, 0.0, 1.0), 1.0);
}`,
  },
  {
    name: 'Waveform distortion',
    mode: 'shadertoy',
    source: `// Original example (LGPL-2.1). Scanline bent by the waveform (row y=0.75).
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float wav = texture(iChannel0, vec2(uv.x, 0.75)).x - 0.5;
    float line = smoothstep(0.03, 0.0, abs(uv.y - 0.5 - wav * 0.5));
    vec3 col = vec3(0.2, 0.9, 0.6) * line + 0.04;
    col += 0.3 * vec3(iBass, iMid, iTreb);
    fragColor = vec4(col, 1.0);
}`,
  },
  {
    name: 'Raymarched sphere',
    mode: 'shadertoy',
    source: `// Original example (LGPL-2.1). Minimal SDF raymarch; radius pulses with iBass.
float sdSphere(vec3 p, float r) { return length(p) - r; }

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;
    vec3 ro = vec3(0.0, 0.0, -3.0);
    vec3 rd = normalize(vec3(uv, 1.5));
    float t = 0.0;
    for (int i = 0; i < 48; i++) {
        vec3 p = ro + rd * t;
        float d = sdSphere(p, 1.0 + 0.15 * iBass);
        if (d < 0.001 || t > 10.0) break;
        t += d;
    }
    vec3 col = vec3(0.02, 0.02, 0.05);
    if (t < 10.0) {
        vec3 p = ro + rd * t;
        vec3 n = normalize(p);
        float diff = max(0.0, dot(n, normalize(vec3(0.5, 0.8, -0.6))));
        col = (0.2 + diff) * (0.5 + 0.5 * cos(vec3(0.0, 2.0, 4.0) + iTime + p.y));
    }
    fragColor = vec4(col, 1.0);
}`,
  },
];
