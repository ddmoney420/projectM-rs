// Original multipass Shadertoy examples (Phase 8d). All code is original — no
// third-party Shadertoy source. Each demonstrates a piece of the buffer graph:
// Previous-Self feedback, cross-buffer flow, the audio texture, and beat/tempo
// uniforms. Channels are [iChannel0..3].

export interface MultipassPass {
  type: 'buffera' | 'bufferb' | 'bufferc' | 'bufferd' | 'image';
  source: string;
  channels: [string, string, string, string];
}
export interface MultipassExample {
  name: string;
  passes: MultipassPass[];
}

export const MULTIPASS_EXAMPLES: MultipassExample[] = [
  {
    name: 'Feedback Paint (mouse)',
    passes: [
      {
        type: 'buffera',
        source: `// Buffer A: accumulate paint under the mouse, fading over time.
// iChannel0 = Self (previous frame), iChannel1 = Audio.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec4 prev = texture(iChannel0, uv);
    vec2 m = iMouse.xy / iResolution.xy;
    float paint = (iMouse.z > 0.0) ? smoothstep(0.06, 0.0, distance(uv, m)) : 0.0;
    vec3 col = prev.rgb * 0.99 + paint * vec3(1.0, 0.6, 0.2);
    fragColor = vec4(col, 1.0);
}
`,
        channels: ['self', 'audio', 'none', 'none'],
      },
      {
        type: 'image',
        source: `// Image: show Buffer A.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = texture(iChannel0, fragCoord / iResolution.xy);
}
`,
        channels: ['buffera', 'none', 'none', 'none'],
      },
    ],
  },
  {
    name: 'Audio Trails',
    passes: [
      {
        type: 'buffera',
        source: `// Buffer A: inject an audio-driven line, leave decaying trails.
// iChannel0 = Self, iChannel1 = Audio.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec4 prev = texture(iChannel0, uv);
    float fft = texture(iChannel1, vec2(uv.x, 0.25)).x;
    float line = smoothstep(0.02, 0.0, abs(uv.y - 0.5 - fft * 0.35));
    vec3 col = prev.rgb * 0.95 + line * vec3(0.2, 0.8, 1.0) * (0.4 + iVol);
    fragColor = vec4(col, 1.0);
}
`,
        channels: ['self', 'audio', 'none', 'none'],
      },
      {
        type: 'image',
        source: `// Image: gamma-lift Buffer A.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec4 c = texture(iChannel0, fragCoord / iResolution.xy);
    fragColor = vec4(pow(c.rgb, vec3(0.8)), 1.0);
}
`,
        channels: ['buffera', 'none', 'none', 'none'],
      },
    ],
  },
  {
    name: 'Two-Buffer Flow',
    passes: [
      {
        type: 'buffera',
        source: `// Buffer A: a procedural interference field.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float v = sin(uv.x * 8.0 + iTime) + cos(uv.y * 8.0 + iTime * 0.7);
    fragColor = vec4(0.5 + 0.5 * sin(v), 0.5 + 0.5 * cos(v), 0.5, 1.0);
}
`,
        channels: ['none', 'none', 'none', 'none'],
      },
      {
        type: 'bufferb',
        source: `// Buffer B: sample Buffer A (this frame) with a small drift.
// iChannel0 = Buffer A.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec2 off = vec2(sin(iTime), cos(iTime)) * 0.02;
    fragColor = texture(iChannel0, uv + off);
}
`,
        channels: ['buffera', 'none', 'none', 'none'],
      },
      {
        type: 'image',
        source: `// Image: show Buffer B.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = texture(iChannel0, fragCoord / iResolution.xy);
}
`,
        channels: ['bufferb', 'none', 'none', 'none'],
      },
    ],
  },
  {
    name: 'Audio Feedback Tunnel',
    passes: [
      {
        type: 'buffera',
        source: `// Buffer A: zoom the previous frame toward the center on each beat and
// inject a radial audio ring. iChannel0 = Self, iChannel1 = Audio.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec2 c = uv - 0.5;
    vec2 z = c * (0.98 - 0.03 * iBeatPulse) + 0.5;
    vec4 prev = texture(iChannel0, z);
    float fft = texture(iChannel1, vec2(length(c) * 1.5, 0.25)).x;
    vec3 col = prev.rgb * 0.95 + fft * vec3(1.0, 0.4, 0.8) * (0.4 + iBass);
    fragColor = vec4(col, 1.0);
}
`,
        channels: ['self', 'audio', 'none', 'none'],
      },
      {
        type: 'image',
        source: `// Image: show Buffer A.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = texture(iChannel0, fragCoord / iResolution.xy);
}
`,
        channels: ['buffera', 'none', 'none', 'none'],
      },
    ],
  },
];
